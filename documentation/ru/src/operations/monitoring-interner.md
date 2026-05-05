# Мониторинг query interner

Query interner дедуплицирует тексты Parse в памяти процесса
pg_doorman. Хранилище разделено на две независимые хеш-таблицы,
NAMED и ANON; каждая работает по своей политике. NAMED очищает
пассивный GC по `Arc::strong_count`. ANON выселяет по бездействию
через `query_interner_anon_idle_ttl_seconds`. Обе половины
публикуют метрики Prometheus (gauges, eviction-counters, гистограмму
длительности sweep'а) и счётчик синтетических SQLSTATE 26000 —
этот код pg_doorman возвращает клиентам, чей анонимный prepared
statement выпал из всех кешей.

Эта страница помогает оператору пользоваться этими метриками:
рецепт дашборда, правила алертов и приёмы настройки.

## Дашборд

### Главные панели (на первом экране)

1. **Stat — общий объём интернера.**
   `sum(pg_doorman_query_interner_bytes)` в разрезе инстансов.
   Красный порог 1.5 ГиБ, жёлтый — 500 МиБ. Главный сигнал по
   памяти.
2. **Time series — entries по kind.** Две линии:
   - `pg_doorman_query_interner_entries{kind="named"}`
   - `pg_doorman_query_interner_entries{kind="anonymous"}`
   Окно шесть часов. Устойчивый рост любой из линий — повод открыть
   панели детализации.
3. **Time series — частота синтетических 26000.**
   `rate(pg_doorman_query_interner_synthetic_misses_total[5m])`.
   Норма — плоский ноль. Любой всплеск означает, что TTL вытеснил
   запись, на которую сослался клиент, или драйвер рассчитывает на
   unnamed prepared statement, переживший Sync.

### Детализация

4. Скорость eviction'ов с разбивкой по reason:
   `sum by (kind, reason) (rate(pg_doorman_query_interner_evictions_total[5m]))`.
5. Heatmap длительности sweep'а:
   `histogram_quantile(0.5, rate(pg_doorman_query_interner_gc_duration_seconds_bucket[5m]))`,
   с P99 поверх.
6. Среднее число байт на запись по kind:
   `pg_doorman_query_interner_bytes / pg_doorman_query_interner_entries`.

### Корреляции

7. Скорость eviction'ов anon vs общий query rate. Линейная
   корреляция говорит о здоровом трафике; нелинейная — о взрыве
   динамического SQL от ORM.
8. Частота синтетических 26000 vs P99 latency запросов. Корреляция
   означает, что TTL режет реальный трафик; разбираться с медленным
   путём.

### Переменные дашборда

- `instance` — сравнивать реплики.
- `kind` — отрезать gauges и counter до одной из половин.

Pool, user и database к интернеру неприменимы — он один на процесс.
Эти лейблы на interner-панелях только введут читателя в
заблуждение.

## Правила алертов

Готовый блок `groups:` лежит в
`monitoring/prometheus-rules/pg_doorman_interner.yaml`. Пять
алертов.

- **`PgDoormanAnonInternerMemoryHigh`** (critical) — ANON bytes
  выше 1.5 ГиБ. Уменьшить TTL или проверить ORM на динамический
  SQL.
- **`PgDoormanAnonTTLTooShort`** (critical) — синтетические 26000
  чаще 1/с в течение 10 минут. Поднять TTL или починить виновный
  драйвер.
- **`PgDoormanAnonInternerNotShrinking`** (warning) — ANON растёт,
  а TTL-eviction плоский. TTL слишком велик, либо поток уникальных
  запросов превышает скорость их истечения.
- **`PgDoormanInternerGCSlow`** (warning) — P99 sweep'а выше 50 мс
  на 15-минутном окне. Увеличить
  `query_interner_gc_interval_seconds` или сделать
  `RESET INTERNER` и уменьшить размеры кешей.
- **`PgDoormanNamedInternerGrowsUnbounded`** (warning) — больше
  100k записей в NAMED при почти нулевом eviction. Почти всегда баг,
  при котором `Arc<str>` strong-ref удерживается навсегда.

Защита от холодного старта: у всех алертов `for: > 5m`. Пустой
интернер сразу после запуска процесса их не зажигает.

## Приёмы настройки

### Уменьшить TTL, когда давит память

Когда: горит `PgDoormanAnonInternerNotShrinking`, ANON bytes
подходит к лимиту памяти хоста.

Действие: уменьшить `query_interner_anon_idle_ttl_seconds` в
`general` (например, с 60 до 30). Перечитать конфиг pg_doorman.
Скорость вытеснений догонит новый порог.

### Поднять TTL при синтетических 26000

Когда: горит `PgDoormanAnonTTLTooShort`.

Действие: понять, какой клиент и какой запрос. У synthetic_misses
нет лейблов, поэтому смотреть WARN-лог — он пишется на каждый
миссинг и содержит client, pool, connection_id. Если виновник —
драйвер, который законно переиспользует unnamed Bind в следующем
batch'е, поднять TTL (с 60 до 300, например). Если нет —
переключить клиента на named prepared.

### Сбросить интернер

Когда: разовая диагностика или жёсткое сжатие памяти под инцидентом.

Действие: `psql admin@:6432 -c "RESET INTERNER"`. Возвращает
`CommandComplete RESET`. Активные клиенты переparsят при следующем
использовании. Короткоживущие клиенты эффекта не заметят: их
`last_anonymous_hash` всё ещё помнит хеш, который они
зарегистрировали до сброса, и следующий Bind увидит отсутствие
записи и один раз получит 26000. Драйвер на это отреагирует
повторным Parse.

## Recording rules

Кластерные агрегаты, которые имеет смысл предвычислять, чтобы
дашборды были дешевле:

```yaml
groups:
  - name: pg_doorman_query_interner_recording
    interval: 30s
    rules:
      - record: pg_doorman:query_interner_total_bytes:5m
        expr: sum without (instance) (pg_doorman_query_interner_bytes)
      - record: pg_doorman:query_interner_eviction_rate:5m
        expr: |
          sum without (instance) (rate(pg_doorman_query_interner_evictions_total[5m]))
```

Первое правило позволяет кластерной stat-панели читать одну серию.
Второе рисует eviction-rate с разбивкой по reason без пересчёта
`rate()` на каждом перерендере дашборда.
