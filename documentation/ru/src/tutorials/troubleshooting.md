# Диагностика

Симптомы, на которые вы скорее всего наступите в первую неделю работы PgDoorman, и куда смотреть, когда наступили.

## Ошибки аутентификации при подключении к PostgreSQL

**Симптом:** PgDoorman принимает клиентское соединение, но первый же запрос возвращает `password authentication failed` от PostgreSQL.

### Когда username пула совпадает с ролью на backend

PgDoorman по умолчанию использует **passthrough authentication** — криптографическое доказательство клиента (MD5-хеш или SCRAM `ClientKey`) повторно используется для аутентификации к PostgreSQL. Поле `password` в конфиге должно содержать именно тот хеш, что хранится в `pg_authid` / `pg_shadow`:

```sql
SELECT usename, passwd FROM pg_shadow WHERE usename = 'your_user';
```

Для SCRAM оба процесса должны видеть **одни и те же** salt и iteration count — даже один отличающийся символ в сохранённом verifier ломает passthrough.

### Когда username пула отличается от роли на backend

Когда обращённый к клиенту `username` в PgDoorman не совпадает с реальной ролью PostgreSQL, passthrough работать не может — нечего пробрасывать. Дайте явные credentials:

```yaml
users:
  - username: "app_user"              # имя для клиента
    password: "md5..."                # хеш для аутентификации client → pg_doorman
    server_username: "pg_app_user"    # реальная роль в PostgreSQL
    server_password: "plaintext_pwd"  # plaintext-пароль для этой роли
    pool_size: 40
```

Это же путь для JWT-аутентификации, где клиент не присылает пароль и пробрасывать нечего.

```admonish tip title="Где взять хеш пароля"
`pg_doorman generate --host …` интроспектирует PostgreSQL и собирает конфиг с уже подставленными хешами. Быстрее, чем копировать руками из `pg_shadow`.
```

## Файл конфигурации не найден

**Симптом:** PgDoorman при запуске завершается с `configuration file not found`.

По умолчанию бинарник ищет `pg_doorman.toml` в текущем рабочем каталоге. Либо назовите файл так и `cd` в его каталог, либо передайте путь явно:

```bash
pg_doorman /etc/pg_doorman/pg_doorman.yaml
```

Проверка перед запуском:

```bash
pg_doorman -t /etc/pg_doorman/pg_doorman.yaml
```

## Клиенты получают `58006` (`pooler is shut down now`)

Пул выключается, либо binary upgrade был запущен в daemon mode. Посмотрите серверные логи рядом с timestamp ошибки:

- `Got SIGUSR2, starting binary upgrade …` — идёт binary upgrade. В foreground mode idle-клиенты должны мигрировать прозрачно; `58006` получают только клиенты, оставшиеся в транзакции после `shutdown_timeout`. В daemon mode fd-based миграции нет, и каждый клиент получает `58006` при закрытии соединения. См. [Binary upgrade → Troubleshooting](binary-upgrade.md#troubleshooting).
- Нет строки `SIGUSR2` в логе — кто-то прислал `SIGTERM` или `SIGINT`, и пулер выключился без замены. Проверьте systemd-юнит, конкретный pid и operator runbook.

Если `58006` пришёл во время плановой замены — для этой подмножества клиентов это ожидаемое поведение. Настройте connection pool приложения на retry при transient-ошибках.

## Pool size слишком мал

**Симптом:** запросы ходят заметно дольше end-to-end, чем при прямом обращении к PostgreSQL.

Смотрите `SHOW POOLS` и `SHOW POOLS_EXTENDED`:

```
cl_waiting   — сколько клиентов сейчас в очереди за backend
maxwait      — самое долгое ожидание любого waiter, секунд
sv_idle      — idle backend-ов в пуле
sv_active    — backend-ов, выданных клиентам
```

Если `cl_waiting > 0` стабильно и `sv_idle == 0`, пул мал для нагрузки. Либо поднимайте `pool_size` для этого пользователя, либо разбирайтесь, почему `sv_active` не падает — длинные транзакции, idle-in-transaction сессии, медленный downstream-вызов, который держит backend.

Если у вас включён `max_db_connections`, смотрите ещё `SHOW POOL_COORDINATOR` на `evictions` (доноры под давлением отдают соединения) и `exhaustions` (лимит достигнут даже после вытеснений). См. [Pool Coordinator](../concepts/pool-coordinator.md).

## Куда подавать остальное

```admonish tip title="Проблема осталась?"
Если вашей проблемы здесь нет — [откройте issue на GitHub](https://github.com/ozontech/pg_doorman/issues): версия pg_doorman, релевантный конфиг (с замазанными паролями), драйвер клиента и его версия, и совпадающие по времени строки логов из pg_doorman и PostgreSQL.
```
