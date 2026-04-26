# Аутентификация JWT

Аутентифицируйте клиентов JSON Web Token, подписанным внешним поставщиком идентификации. pg_doorman проверяет подпись токена RSA-SHA256 по публичному ключу с диска, сверяет claim `preferred_username` и пробрасывает соединение в PostgreSQL под заданной идентичностью бэкенда.

Этот метод подходит для доступа сервиса к базе, когда короткоживущие токены выпускает OIDC-провайдер, Vault или внутренний токен-сервис.

## Конфигурация

Сгенерируйте (или получите) публичный RSA-ключ и сошлитесь на него в поле `password` пользователя через префикс `jwt-pkey-fpath:`:

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "billing-service"
        password: "jwt-pkey-fpath:/etc/pg_doorman/jwt-public.pem"
        server_username: "billing"
        server_password: "md5..."
        pool_size: 40
```

То, что клиент пришлёт как пароль, будет считаться JWT и проверяться по `/etc/pg_doorman/jwt-public.pem`. Токен должен:

- Быть подписан RS256 (RSA-SHA256). HS256 и EC-варианты не поддерживаются.
- Иметь claim `preferred_username`, равный заданному `username` (`billing-service` в примере выше).
- Проходить стандартную валидацию `exp` и `nbf`.

Соединение с бэкендом открывается как `billing` с хешем из `server_password`. Идентичность клиента (`billing-service`) отвязана от идентичности базы (`billing`).

## Генерация пары ключей

```bash
openssl genrsa -out jwt-private.pem 2048
openssl rsa -in jwt-private.pem -pubout -out jwt-public.pem
```

Храните `jwt-private.pem` у издателя токенов. Раздавайте `jwt-public.pem` в pg_doorman.

## Выпуск токена

Подойдёт любая RS256 JWT-библиотека. Пример на Python (`PyJWT`):

```python
import jwt
import time

private_key = open("jwt-private.pem").read()

token = jwt.encode(
    {
        "preferred_username": "billing-service",
        "iat": int(time.time()),
        "exp": int(time.time()) + 300,  # 5 минут
    },
    private_key,
    algorithm="RS256",
)
```

Клиент подключается к pg_doorman с `user=billing-service` и `password=<token>`. Большинство драйверов PostgreSQL принимают любую строку в поле пароля.

## Ротация токенов

pg_doorman читает файл публичного ключа один раз при старте и по `SIGHUP`. Чтобы ротировать ключ:

1. Добавьте новый публичный ключ во вторую запись пользователя с параллельным именем.
2. Сделайте reload (`kill -HUP`).
3. Переключите издателя на новый ключ.
4. Удалите старую запись пользователя после grace-периода.

Или, проще, замените файл на месте и пошлите `SIGHUP`. Поддержки нескольких ключей на одного пользователя нет.

## Порядок выбора метода

JWT — самый низкоприоритетный формат пароля: pg_doorman сначала проверяет префиксы `SCRAM-SHA-256$` и `md5`, затем `jwt-pkey-fpath:`. На практике это важно только если вы используете пароль-заглушку — задайте `auth_pam_service` для PAM или используйте префикс `jwt-pkey-fpath:` исключительно для JWT-пользователей.

Если у одного и того же пользователя заданы и `auth_pam_service`, и пароль `jwt-pkey-fpath:`, выигрывает PAM.

Смотрите [Обзор](overview.md#порядок-выбора-метода).

## Оговорки

- Claim `preferred_username` должен совпадать в точности. Сопоставлений или алиасов claim'ов нет.
- Поддержки JWKS-эндпоинта нет: публичный ключ должен быть на диске.
- Проверки издателя (`iss`) или аудитории (`aud`) нет. Если нужны — терминируйте JWT в sidecar и переводите в passthrough-аутентификацию.
- Если идентичность клиента должна нести информацию о роли в базе (например, `read_only` против `read_write`), смотрите [Talos](talos.md).
