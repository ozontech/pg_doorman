# Структурированное JSON-логирование

pg_doorman пишет структурированные JSON-логи при запуске с `--log-format structured`. Каждая строка — самодостаточный JSON-объект с timestamp, уровнем, местом в исходниках и сообщением, готовый к приёму в Loki, Elasticsearch, Datadog или любой пайплайн логов, ожидающий JSON.

## Включение

Три равноценных способа:

```bash
# Флаг командной строки
pg_doorman -F structured /etc/pg_doorman/pg_doorman.yaml

# Длинная форма
pg_doorman --log-format structured /etc/pg_doorman/pg_doorman.yaml

# Переменная окружения
LOG_FORMAT=structured pg_doorman /etc/pg_doorman/pg_doorman.yaml
```

По умолчанию — `text` (человекочитаемый). Флаг `--log-format` принимает значения `text`, `structured` или `debug`; последнее на данный момент является алиасом для `text`.

## Формат вывода

```json
{"timestamp":"2026-04-25T08:32:14.512Z","level":"INFO","file":"src/app/server.rs","line":357,"message":"Server is up at 0.0.0.0:6432"}
{"timestamp":"2026-04-25T08:32:14.514Z","level":"INFO","file":"src/pool/mod.rs","line":421,"message":"Pool 'mydb' initialized: 1 user, pool_size=40"}
{"timestamp":"2026-04-25T08:32:18.103Z","level":"WARN","file":"src/server/protocol_io.rs","line":189,"message":"Backend connection lost: connection reset by peer"}
```

Поля:

| Поле | Тип | Примечания |
| --- | --- | --- |
| `timestamp` | строка RFC 3339 | UTC, точность до миллисекунд. |
| `level` | строка | `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`. |
| `file` | строка | Файл исходника, который пишет лог. |
| `line` | целое | Номер строки. |
| `message` | строка | Человекочитаемое сообщение. |

Вложенных полей и меток на событие нет — логгер pg_doorman сериализует обычные события макроса `log` в JSON. Для богатых метаданных (счётчики на пул, события на клиент) используйте Prometheus-метрики. См. [Prometheus reference](../reference/prometheus.md).

## Уровень логирования

Задаётся через `general.log_level` в конфиге или переопределяется при старте:

```yaml
general:
  log_level: "info"
```

```bash
pg_doorman -l debug -F Structured /etc/pg_doorman/pg_doorman.yaml
```

Изменение в рантайме через admin-базу:

```sql
SET log_level = 'debug';
SHOW LOG_LEVEL;
```

Это влияет только на текущий процесс. Чтобы изменения сохранялись, отредактируйте конфиг и выполните `RELOAD` или отправьте `SIGHUP`.

## Рекомендуемый пайплайн

Для Kubernetes:

```yaml
spec:
  containers:
    - name: pg_doorman
      image: ghcr.io/ozontech/pg_doorman:latest
      args:
        - "-F"
        - "Structured"
        - "/etc/pg_doorman/pg_doorman.yaml"
      env:
        - name: LOG_LEVEL
          value: "info"
```

Логи идут в stdout, рантайм контейнера их захватывает, ваш log shipper (Promtail, Fluent Bit, Vector) пересылает их как есть — JSON сохраняется на всём пути.

Для systemd:

```ini
[Service]
ExecStart=/usr/bin/pg_doorman -F Structured /etc/pg_doorman/pg_doorman.yaml
StandardOutput=journal
StandardError=journal
```

`journalctl -u pg_doorman -o json` возвращает JSON обратно.

## Оговорки

- Для production выбирайте `Text` (терминалы, syslog) или `Structured` (log shippers). `Debug` зарезервирован под будущее использование и сейчас равен `Text`.
- `file` и `line` берутся из мест вызова макроса `log`. Они доступны в release-сборках, потому что pg_doorman поставляется с включённой отладочной информацией.
- Логгер не включает trace-идентификаторы и корреляцию запросов. Для трассировки на запрос используйте `SHOW CLIENTS` и Prometheus-метрики.

## Куда дальше

- [Prometheus reference](../reference/prometheus.md) — для машинно-читаемых метрик.
- [Latency Percentiles](percentiles.md) — для сигналов о производительности.
- [Admin Commands](admin-commands.md) — для интроспекции в рантайме.
