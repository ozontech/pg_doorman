# Аутентификация PAM

pg_doorman делегирует аутентификацию клиента сервису PAM на хосте. Используйте это для аутентификации, интегрированной с OS (LDAP через `pam_ldap`, Kerberos, локальные модули PAM), без хранения учётных данных на каждого пользователя в конфиге пула.

PAM работает только под Linux. Готовые бинарники собираются с поддержкой PAM.

## Конфигурация

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "alice"
        auth_pam_service: "pg_doorman"
        server_username: "alice"
        server_password: "md5..."
        pool_size: 20
```

`auth_pam_service` — имя файла сервиса PAM в `/etc/pam.d/`. pg_doorman не проверяет имя сервиса при старте — убедитесь, что файл существует.

Поле `password` опускается, потому что проверкой занимается PAM. `server_username` и `server_password` обязательны: PAM аутентифицирует только клиента в pg_doorman; pg_doorman всё равно нужны учётные данные для соединения с бэкендом.

## Пример сервиса PAM

`/etc/pam.d/pg_doorman`:

```
auth     required pam_unix.so
account  required pam_unix.so
```

Для аутентификации через LDAP:

```
auth     required pam_ldap.so
account  required pam_ldap.so
```

Настройте `pam_ldap` в `/etc/ldap.conf` (или `/etc/nslcd.conf`) под своё окружение.

## Порядок выбора метода

PAM проверяется после Talos и HBA Trust, но до любого метода на основе пароля. Если у пользователя одновременно заданы `auth_pam_service` и статический `password` (с префиксом MD5, SCRAM или JWT), выигрывает PAM.

Смотрите [Обзор](overview.md#порядок-выбора-метода).

## Оговорки

- PAM блокирует поток-обработчик во время вызова аутентификации. Если ваш стек PAM делает сетевые вызовы (LDAP, Kerberos), ждите эпизодических всплесков задержки.
- `pam_unix.so` требует доступ на чтение к `/etc/shadow` — обычно только для `root`. Запускайте pg_doorman под пользователем с нужным членством в группе или используйте другой модуль PAM.
- PAM не поддерживает passthrough SCRAM. Соединение с бэкендом всегда использует `server_username` и `server_password`.
- Для LDAP без машинерии PAM в pg_doorman нет нативной поддержки LDAP. Используйте Odyssey или PgBouncer 1.25+.
