# Диагностика

Это руководство помогает решить типичные проблемы при использовании PgDoorman.

## Ошибки аутентификации при подключении к PostgreSQL

**Симптом:** PgDoorman запустился успешно, но клиенты получают ошибки аутентификации вроде `password authentication failed` при попытке выполнить запрос.

### Если username пула совпадает с пользователем backend-PostgreSQL

PgDoorman по умолчанию использует **passthrough authentication** -- криптографическое доказательство клиента (MD5-хэш или SCRAM ClientKey) повторно используется для аутентификации в PostgreSQL. Убедитесь, что поле `password` в конфиге содержит именно тот хэш, что хранится в `pg_authid` / `pg_shadow`:

```bash
SELECT usename, passwd FROM pg_shadow WHERE usename = 'your_user';
```

Скопируйте хэш (например, `md5...` или `SCRAM-SHA-256$...`) в поле `password` конфига. Хэш **должен совпадать** с тем, что хранится в PostgreSQL (та же соль и количество итераций для SCRAM).

### Если username пула отличается от backend-пользователя

Когда обращённый к клиенту `username` в PgDoorman не совпадает с реальной ролью PostgreSQL, passthrough работать не может -- нужны явные credentials:

```yaml
users:
  - username: "app_user"              # имя для клиента
    password: "md5..."                # хэш для аутентификации клиента
    server_username: "pg_app_user"    # реальная роль в PostgreSQL
    server_password: "plaintext_pwd"  # plaintext-пароль для этой роли
    pool_size: 40
```

Это же касается JWT-аутентификации, где нет пароля для passthrough.

```admonish tip title="Как получить хэш пароля"
Хэши паролей пользователей можно получить из PostgreSQL запросом: `SELECT usename, passwd FROM pg_shadow;`

Или используйте команду `pg_doorman generate`, которая получает их автоматически.
```

## Файл конфигурации не найден

**Симптом:** PgDoorman завершается с ошибкой "configuration file not found".

**Решение:** укажите путь к файлу конфигурации явно:

```bash
pg_doorman /path/to/pg_doorman.yaml
```

По умолчанию PgDoorman ищет `pg_doorman.toml` в текущей директории.

## Pool size слишком мал

**Симптом:** клиенты долго ждут или получают ошибки про слишком большое количество соединений.

**Решение:** увеличьте `pool_size` для затронутого пользователя или проверьте значения `cl_waiting` и `maxwait` в admin-команде `SHOW POOLS`. Если `maxwait` стабильно высокий, значит пул мал для вашей нагрузки.

---

```admonish tip title="Проблема осталась?"
Если столкнулись с проблемой, которой здесь нет, [откройте issue на GitHub](https://github.com/ozontech/pg_doorman/issues).
```
