# Мониторинг query interner

Query interner дедуплицирует тексты Parse в памяти процесса
pg_doorman. Две половины работают по разным политикам: NAMED
ограничен пассивным GC по `Arc::strong_count`, ANON — per-entry
TTL по бездействию (`query_interner_anon_idle_ttl_seconds`). Обе
половины публикуют Prometheus-метрики (gauges, eviction-counters,
гистограмму длительности sweep'а), плюс счётчик синтетических
SQLSTATE 26000, которые pg_doorman возвращает клиентам, чей
анонимный prepared statement выпал из всех кешей.

Эта страница — operator-сторона тех метрик: рецепт дашборда,
правила алертов и руководство по настройке.

## Дашборд

### Above-the-fold (три верхние панели)

1. **Stat — total bytes интернера.**
   `sum(pg_doorman_query_interner_bytes)` per-instance, красный
   порог 1.5 GiB, жёлтый — 500 MiB. Главный сигнал по памяти.
2. **Time series — entries по kind.** Две линии:
   - `pg_doorman_query_interner_entries{kind="named"}`
   - `pg_doorman_query_interner_entries{kind="anonymous"}`
   Окно 6 часов. Устойчивый рост любой из линий — повод открыть
   drill-down.
3. **Time series — синтетический 26000 rate.**
   `rate(pg_doorman_query_interner_synthetic_misses_total[5m])`.
   Норма — плоский ноль. Любой всплеск означает, что TTL вытеснил
   запись, на которую ссылался клиент, или драйвер опирался на
   cross-batch unnamed.

### Drill-down

4. Eviction rate с разбивкой по reason:
   `sum by (kind, reason) (rate(pg_doorman_query_interner_evictions_total[5m]))`
5. Heatmap длительности sweep'а:
   `histogram_quantile(0.5, rate(pg_doorman_query_interner_gc_duration_seconds_bucket[5m]))`,
   с P99-линией поверх.
6. Среднее число байт на запись:
   `pg_doorman_query_interner_bytes / pg_doorman_query_interner_entries`,
   per-kind.

### Корреляции

7. Anon eviction rate vs total query rate. Линейная корреляция —
   нормальный трафик; нелинейная — взрыв динамического SQL от ORM.
8. Synthetic 26000 rate vs P99 query latency. Корреляция — TTL
   режет реальный трафик; разбираться с медленным путём.

### Рекомендуемые dashboard variables

- `instance` — для сравнения реплик.
- `kind` — для среза gauges/counters до одной половины.

Поля pool/user/database к интернеру неприменимы — он глобален на
процесс. Их добавление к interner-панелям ввело бы читателя в
заблуждение.

## Правила алертов

Полный `groups:` блок поставляется по адресу
`monitoring/prometheus-rules/pg_doorman_interner.yaml`. Пять
алертов:

- **`PgDoormanAnonInternerMemoryHigh`** (critical) — ANON bytes
  > 1.5 GiB. Уменьшить TTL или проверить ORM на динамический SQL.
- **`PgDoormanAnonTTLTooShort`** (critical) — synthetic 26000 rate
  > 1/s 10 минут. Поднять TTL или починить виновный драйвер.
- **`PgDoormanAnonInternerNotShrinking`** (warning) — ANON растёт,
  TTL-eviction плоский. TTL слишком велик или поток уникальных
  запросов выше ритма истечения.
- **`PgDoormanInternerGCSlow`** (warning) — sweep P99 > 50 ms 15
  минут. Увеличить `query_interner_gc_interval_seconds` или
  выполнить `RESET INTERNER` плюс уменьшить размеры кешей.
- **`PgDoormanNamedInternerGrowsUnbounded`** (warning) — NAMED
  > 100k записей с почти нулевым eviction. Почти всегда баг,
  где Arc<str> strong-ref остаётся жив навсегда.

Cold-start guard: все алерты используют `for: > 5m`, поэтому пустой
интернер сразу после запуска процесса их не зажигает.

## Рецепты настройки

### Уменьшить TTL, когда давит память

Триггер: `PgDoormanAnonInternerNotShrinking`, ANON bytes
приближается к budget'у хоста.

Действие: уменьшить `query_interner_anon_idle_ttl_seconds` в
`general` (например, 60 → 30). Reload pg_doorman. Скорость
вытеснений догонит новый порог.

### Поднять TTL при синтетических 26000

Триггер: `PgDoormanAnonTTLTooShort`.

Действие: определить клиента и запрос. У synthetic_misses нет
labels, поэтому смотрите WARN-лог, который пишется при каждом
миссе с client / pool / connection_id. Если виновник — драйвер,
законно переиспользующий unnamed Bind через batch, поднять TTL
(например, 60 → 300). Если нет — переключить клиента на named
prepared.

### Запустить RESET INTERNER

Триггер: ad-hoc диагностика или контейнинг инцидента по памяти.

Действие: `psql admin@:6432 -c "RESET INTERNER"`. Возвращает
`CommandComplete RESET`. Активные клиенты делают повторный Parse
при следующем использовании; короткоживущие клиенты эффект не
ощущают, потому что их `last_anonymous_hash` помнит хеш, который
они зарегистрировали до сброса, и следующий Bind обнаруживает
отсутствие записи и один раз отвечает 26000 — драйвер делает
повторный Parse.

## Recording rules

Cluster-wide агрегаты, которые имеет смысл предвычислять для более
дешёвых дашбордов:

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

Первое правило позволяет cluster-wide stat-панели читать одну
серию; второе — рисовать eviction-rate-by-reason без повторного
вычисления `rate()` на каждом перерендере.
