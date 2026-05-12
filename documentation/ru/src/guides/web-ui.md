# Веб-консоль

В pg_doorman встроена операторская веб-консоль. Она работает на том же
HTTP-сервере, что отдаёт Prometheus-метрики; собранный фронтенд лежит
внутри бинарника. Запуск консоли не добавляет внешних зависимостей: один
процесс, один бинарь, один TCP-порт.

## Включение

Консоль настраивается в секции `[web]`. Старое имя секции `[prometheus]`
тоже принимается как алиас.

```toml
[web]
enabled = true
host = "0.0.0.0"
port = 9127

# Операторская консоль (по умолчанию выключена)
ui = true
ui_anonymous = false
log_tap_max_entries = 8192
```

При `web.ui = true` и `general.admin_password`, равном пустой строке или
литералу `"admin"`, консоль на старте переходит в режим «только
метрики». HTTP-сервер продолжает отдавать `/metrics`, но административные эндпоинты
иначе оказались бы открыты любому. Задайте настоящий пароль до того, как
включать `ui = true`. Срабатывание этой проверки видно в логе по строке
`web.ui = true ignored: admin_password is default/empty`.

| Параметр | Описание | По умолчанию |
|---|---|---|
| `enabled` | Запускать ли HTTP-сервер. `/metrics` работает независимо от `ui`. | `false` |
| `host` | Адрес, на котором слушает HTTP-сервер. | `"0.0.0.0"` |
| `port` | Порт HTTP-сервера. | `9127` |
| `ui` | Отдавать SPA по `/` и публичные API-эндпоинты. | `false` |
| `ui_anonymous` | При `true` публичные API-эндпоинты принимают запросы без авторизации. См. [Роли доступа](#роли-доступа). | `false` |
| `log_tap_max_entries` | Размер кольцевого буфера в памяти для `/api/logs`. `0` отключает эндпоинт. | `8192` |

## URL-карта

| URL | Требуемая роль | Назначение |
|---|---|---|
| `/`, `/pools`, любой путь вне API | нет | Оболочка приложения. Отдаётся анонимно даже при `ui_anonymous = false`, чтобы прямая ссылка не открывала системный диалог Basic-авторизации браузера до того, как появится форма входа React. |
| `/assets/*` | нет | Хэшированные JS, CSS, шрифты и SVG. `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | нет | Prometheus exposition format. От `ui` не зависит. |
| `GET /api/auth/config` | нет | Сообщает SPA, подключён ли SSO и какая роль у текущего запроса. |
| `GET /api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/prepared`, `/api/apps`, `/api/events` | `Anonymous`, когда `ui_anonymous = true`, иначе `Sso` | JSON только для чтения, повторяет формат `SHOW <admin-команда>`. |
| `GET /api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top`, `/api/top/queries` | `Sso` | Эндпоинты только для чтения с персональными данными. `/api/logs` подключает буфер логов на первом запросе и отключает его через 2 минуты простоя. `/api/top/queries` возвращает первые ~120 символов SQL-текста из кеша. Эти данные не вынесены в публичную поверхность, потому что превью могут содержать литералы и идентификаторы клиентов. |
| `POST /api/admin/{reload,pause,resume,reconnect}` | `Admin` | Управляющие операции администратора. Семантика та же, что и у admin-протокола через psql. |

## Роли доступа

Сервер на каждом запросе вычисляет одну из трёх ролей. Проверка работает
на стороне сервера; SPA дублирует её на клиенте только для того, чтобы
не показывать действия, недоступные текущему оператору.

| Роль | Как запрос её получает | Что роль даёт |
|---|---|---|
| `Anonymous` | Учётных данных нет, `[web].ui_anonymous = true`. | Публичные `/api/*` только для чтения из таблицы выше плюс `/metrics`. На пути с персональными данными и `/api/admin/*` возвращается `401`. |
| `Sso` | Валидный JWT в `Authorization: Bearer`, в cookie `sso_access_token=` или в query `?token=`, который **не** попадает в admin-группу. | Все эндпоинты чтения, включая пути с персональными данными. На `POST /api/admin/*` отдаётся `403`. |
| `Admin` | Либо корректная пара Basic из `[general].admin_username` / `admin_password`, либо валидный JWT, у которого значение `[web].sso_groups_claim` пересекается с `[web].sso_admin_groups`. | Полный доступ, включая `POST /api/admin/{reload,pause,resume,reconnect}`. |

Когда в одном запросе есть и Basic, и SSO-токен, приоритет у Basic.
Корректный admin-пароль даёт `Admin` независимо от состояния SSO.
Неверный Basic-пароль не блокирует SSO-ветку: SSO-источники всё равно
проверяются, и валидный JWT даёт роль `Sso` (или `Admin`, если совпала
admin-группа). Это покрывает типичный случай: в `localStorage` лежит
просроченный JWT рядом с рабочим Basic-паролем.

Basic-пароль сравнивается за постоянное время, чтобы по длительности
сравнения нельзя было угадывать символы. JWT проверяются по публичному
ключу из `[web].sso_public_key_file`; разобранный ключ кэшируется на
время жизни процесса и перечитывается на `RELOAD`.

`fetch`-обёртка SPA шлёт `Accept: application/json`, и сервер на ней
отдаёт чистый `401` без `WWW-Authenticate: Basic`. Без этого браузер
закешировал бы то, что оператор ввёл в системном диалоге Basic, и
подставлял этот пароль поверх формы входа React. Инструменты с
`Accept: */*` (curl, gh) получают challenge как обычно.

`401 Unauthorized` отдаётся, когда учётных данных не пришло или ни
один вариант не прошёл парсинг и валидацию. `403 Forbidden` — когда
данные валидны, но роли не хватает для пути; тело —
`{"error":"forbidden","message":"admin role required"}`. SPA на `401`
повторно открывает форму входа, на `403` показывает неблокирующий
баннер «admin role required», не уводя на форму входа.

## Настройка SSO

SSO опциональный. По умолчанию (`[web].sso_enabled = false`) сервер
обслуживает только роли `Anonymous` и `Admin` через Basic. Чтобы подключить
внешний SSO-прокси:

1. Получите от SSO-провайдера публичный RSA-ключ, которым он подписывает JWT,
   и сохраните его в PEM-файле (например, `/etc/pg_doorman/sso-public.pem`).
   Для oauth2-proxy ключ извлекается из приватного:
   `openssl rsa -in private.pem -pubout -out public.pem`. Для Keycloak —
   см. [Keycloak](#keycloak) ниже.
2. Добавьте SSO-поля в `[web]`:

   ```toml
   [web]
   enabled = true
   ui = true
   host = "127.0.0.1"
   port = 9127
   ui_anonymous = false

   sso_enabled = true
   sso_proxy_url = "https://sso.example.com/oauth2/start"
   sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
   sso_audience = ["pg_doorman"]
   sso_allowed_users = ["*"]
   ```

3. Перечитайте конфиг: `kill -SIGHUP <pid>` или
   `psql -h <host> -p 6432 -U admin -d pgbouncer -c 'RELOAD'`.
4. Проверьте: `curl http://<host>:9127/api/auth/config` должен вернуть
   `"sso_enabled":true` и заданный `sso_proxy_url`.

| Поле | Назначение | По умолчанию |
|---|---|---|
| `sso_enabled` | Включает SSO-ветку. Без неё JWT не валидируются. | `false` |
| `sso_proxy_url` | URL, на который SPA уводит браузер по кнопке «Sign in via SSO». Бэкенд этот URL сам не вызывает. | `null` |
| `sso_public_key_file` | Путь к PEM-файлу с публичным RSA-ключом. Читается на старте и при `RELOAD`. | `null` |
| `sso_audience` | Допустимые значения claim `aud`. Токен принимается, если совпадает хотя бы одно. Обязательное поле при `sso_enabled = true`. | `[]` |
| `sso_allowed_users` | Allowlist по claim `preferred_username` (или `sub`). `["*"]` принимает любого. Иначе пропускаются только перечисленные имена. | `["*"]` |
| `sso_groups_claim` | Имя JWT-claim, в котором лежат группы пользователя. Читается вместе с `sso_admin_groups`. | `"groups"` |
| `sso_admin_groups` | Группы, которые поднимают SSO-пользователя до `Admin`. Пустой список оставляет каждый SSO-логин на роли `Sso` только для чтения. | `[]` |
| `trusted_proxies` | CIDR доверенных обратных прокси. Пустой список — доверять только непосредственному TCP-peer. См. [Журнал доступа](#журнал-доступа). | `[]` |

### Поднятие SSO-пользователя до Admin через claim с группами

По умолчанию SSO-логин получает роль `Sso` — доступ только для чтения к логам
и SQL-текстам, но без `POST /api/admin/*`. Чтобы операторы могли
запускать управляющие операции администратора через SSO без раздачи Basic-пароля,
настройте `sso_groups_claim` и `sso_admin_groups`:

```toml
[web]
sso_enabled = true
sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
sso_audience = ["pg_doorman"]
sso_groups_claim = "groups"
sso_admin_groups = ["pg-doorman-admins"]
```

Когда в валидном JWT приходит `"groups": [..., "pg-doorman-admins"]`,
запрос получает роль `Admin`. В access-логе это выглядит как
`auth_role=admin auth_source=sso`, и SSO-админы по-прежнему отличимы от
Basic-админов. `/api/auth/config` отдаёт
`sso_admin_groups_configured = true`, и SPA убирает из формы входа
обещание «SSO grants read-only access».

### Keycloak

Keycloak подписывает каждый JWT RSA-ключом realm'а. Публичную часть
этого ключа нужно один раз выгрузить в PEM-файл, который читает
pg_doorman.

Без UI — через JWKS-эндпоинт realm'а:

```bash
REALM=https://kc.example.com/realms/operators
curl -s "$REALM/protocol/openid-connect/certs" \
  | jq -r '.keys[] | select(.alg=="RS256") | "-----BEGIN CERTIFICATE-----\n" + .x5c[0] + "\n-----END CERTIFICATE-----"' \
  | openssl x509 -pubkey -noout \
  > /etc/pg_doorman/sso-public.pem
```

Через админ-консоль: **Realm settings** → **Keys** → строка с
`Algorithm = RS256` и `Use = SIG` → **Public key** → скопированное
base64-тело завернуть в PEM-файл с заголовками
`-----BEGIN PUBLIC KEY-----` / `-----END PUBLIC KEY-----`.

Секция `[web]` под Keycloak выглядит так:

```toml
[web]
sso_enabled = true
sso_proxy_url = "https://kc.example.com/realms/operators/protocol/openid-connect/auth"
sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
sso_audience = ["pg_doorman"]    # client_id, заданный в Keycloak
sso_groups_claim = "groups"      # значение по умолчанию для маппера «groups»
sso_admin_groups = ["pg-doorman-admins"]
```

Чтобы Admin через group claim работал, добавьте клиенту маппер
**Group Membership** (Clients → нужный client → **Mappers**). Без
этого маппера Keycloak выдаёт токены без `groups`, и каждый оператор
остаётся в роли `Sso`.

После ротации ключа realm'а заново выгрузите PEM и сделайте
`RELOAD` — pg_doorman подхватит новый ключ без рестарта.

### Когда SSO-конфигурация сломана

Опечатка в SSO-секции не должна выводить операторскую консоль из строя.
При `sso_enabled = true`, но не загружаемом рантайме (нет PEM-файла,
пустой audience, нечитаемый PEM) сервер пишет причину в лог на уровне
`error`, оставляет SSO выключенным на этот запуск и обслуживает только
Basic и Anonymous. Та же причина видна в двух точках, чтобы оператор
заметил поломку, а не тихий откат на Basic:

- `/api/auth/config.sso_config_error` содержит человекочитаемое
  сообщение. SPA показывает баннер с этим текстом в форме входа.
- Метрика `pg_doorman_web_sso_config_error` равна `1`, пока SSO
  запрошен, но не загружен. В паре с `pg_doorman_web_sso_enabled`
  даёт условие для алерта.

## Логин из браузера

При первом заходе SPA получает `/api/auth/config` и показывает форму
входа. Если в ответе пришёл `sso_proxy_url`, рядом с Basic-формой
появляется кнопка **Sign in via SSO**; иначе — только Basic.

Клик по **Sign in via SSO** уводит браузер на
`${sso_proxy_url}?redirect_to=<текущий URL>`. Внешний proxy выполняет
OAuth/OIDC-обмен и возвращает браузер обратно с `?token=<jwt>`. SPA
сохраняет токен в `localStorage`, чистит URL от параметра и шлёт
`Authorization: Bearer <jwt>` на каждом следующем запросе.

В нижней части боковой панели отображается имя текущего пользователя: `admin`
для Basic или `sso: <preferred_username>` для SSO. Кнопка **Sign out**
очищает в `localStorage` оба ключа (`pgdoorman.admin-auth` и
`pgdoorman.sso-token`) и заново открывает форму входа.

Тихое обновление токена запускается раз в 60 секунд. Когда до `exp` остаётся
меньше 90 секунд, SPA открывает скрытый iframe с URL
`${origin}/?sso_silent=1`. Внутри iframe App-роутер рендерит
минимальный `SilentCallback` (без обычных эффектов опроса), который
через `window.postMessage` отдаёт новый токен parent-окну. Если
тихое обновление не сработало:

- при наличии Basic-данных SPA удаляет SSO-токен без редиректа, и
  дальнейшие запросы идут под Basic;
- иначе SPA уходит на полный редирект через SSO-proxy.

Срок жизни JWT задавайте не меньше 5 минут — более короткие токены
успевают истечь до того, как сработает refresh.

SPA cookie не шлёт (`credentials: "omit"` на каждом fetch). Путь с
cookie `sso_access_token` существует для сайдкаров, curl и oauth2-proxy
вариантов, которые кладут токен в cookie на общем домене.

Basic-пароль по умолчанию живёт только в памяти React и пропадает после
полной перезагрузки страницы. Галочка **Remember me on this device** в форме входа
сохраняет его в `localStorage`, поэтому консоль открывается без повторного ввода.
Очистка хранилища сайта в браузере удаляет и Basic, и SSO-запись.

## Журнал доступа

После каждого ответа (200/401/403/404/5xx, включая скрейпы `/metrics`)
консоль пишет одну logfmt-строку в канал `pg_doorman::web::access`:

```
INFO pg_doorman::web::access method=GET path=/api/admin/reload query=false status=200 bytes=42 latency_ms=12 peer=10.0.1.5:42312 auth_role=admin auth_source=basic auth_user=admin
```

Поля:

- `method`, `path` — HTTP-метод и URL-путь. Тела запросов и ответов в
  лог не пишутся.
- `query=true|false` — был ли в запросе query string. Сама строка не
  логируется, чтобы JWT в `?token=` не попадал в журнал.
- `status`, `bytes`, `latency_ms` — статус ответа, размер тела и
  полная задержка ответа.
- `peer` — адрес инициатора запроса. По умолчанию это непосредственный
  TCP-peer. Если он попадает в `[web].trusted_proxies`, сервер разбирает
  `X-Forwarded-For` (или `Forwarded`, RFC 7239), идёт по цепочке
  справа налево, пропускает доверенные адреса и берёт первый
  недоверенный. Недоверенный клиент не может подделать поле — если
  TCP-peer не в списке доверенных, заголовки прокси игнорируются.
- `auth_role` — `admin`, `sso`, `anonymous` или `rejected`.
- `auth_source` — `basic`, `sso` или `-`.
- `auth_user` — имя пользователя из учётных данных или `-` для
  анонимов и отклонённых запросов.

Уровни:

- `info` — все admin-действия (`POST /api/admin/*`), все чтения
  персональных данных (`/api/logs`, `/api/prepared/text/*`,
  `/api/interner/top`, `/api/top/queries`), все non-2xx ответы и
  любые запросы с авторизованной ролью (`Sso` или `Admin`).
- `debug` — анонимные успешные чтения публичных API и скрейпы
  `/metrics`. Prometheus стучится каждые несколько секунд, SPA
  каждые 1,5 секунды опрашивает overview/pools, поэтому держать эти
  запросы вне `info` важно: иначе `RUST_LOG=info` тонет в шуме.

Отдельный канал `pg_doorman::web::access` позволяет фильтровать поток
журнала доступа независимо от остальных логов. В выпадающем фильтре на
странице **Logs** этот канал включается или исключается одним кликом.

### Реальный IP клиента за обратным прокси

По умолчанию `peer` фиксирует TCP-адрес, который соединился с
сервером. За обратным прокси это адрес самого прокси. Чтобы видеть
реальный IP клиента, добавьте CIDR прокси в `[web].trusted_proxies`:

```toml
[web]
trusted_proxies = ["10.0.0.0/8", "192.168.0.0/16"]
```

Распознаются и `X-Forwarded-For`, и `Forwarded`. Несколько доверенных
переходов в цепочке пропускаются. `X-Forwarded-For`, пришедший от
недоверенного клиента, игнорируется, поэтому через эту настройку
произвольный вызывающий не может управлять полем access-лога.

## Метрики

| Метрика | Тип | Лейблы | Назначение |
|---|---|---|---|
| `pg_doorman_web_sso_enabled` | gauge | — | `1`, когда SSO загружен успешно, иначе `0`. |
| `pg_doorman_web_sso_config_error` | gauge | — | `1`, когда `sso_enabled = true`, но рантайм не поднялся. |
| `pg_doorman_web_auth_attempts_total` | counter | `role`, `source` | Попытки авторизации в разрезе итоговой роли (`admin`/`sso`/`anonymous`/`rejected`) и источника (`basic`/`sso`/`none`). |
| `pg_doorman_web_requests_total` | counter | `status_class`, `role` | Запросы к веб-консоли в разрезе HTTP-статуса (`1xx`–`5xx`) и роли. |
| `pg_doorman_web_sso_validation_errors_total` | counter | `reason` | Отказы валидации JWT по причине: `signature`, `expired`, `audience`, `no_username`, `allowlist`. |

Устойчивый рост `signature` означает, что SSO-прокси ротировал ключ, а
`sso_public_key_file` остался старый. Рост `allowlist` — кто-то снаружи
`sso_allowed_users` упорно пытается войти. Рост `4xx` для роли `sso`
обычно указывает на сломанный прокси перед pg_doorman.

## Диагностика

**`401` на JWT, который должен быть валиден.** Проверьте, что `aud`
совпадает хотя бы с одним значением из `sso_audience` и `exp` ещё не
истёк. PEM проверяется через `openssl rsa -pubin -in <pem> -text -noout`.
Счётчик `pg_doorman_web_sso_validation_errors_total{reason}` показывает,
какая именно проверка не прошла.

**`403` на JWT, который должен быть валиден.** Путь требует роли
`Admin` (например, `POST /api/admin/reload`). Войдите по Basic
admin-паролю или добавьте группу пользователя в
`[web].sso_admin_groups` и перечитайте конфиг.

**SPA не показывает Sign in via SSO.** `/api/auth/config` не возвращает
`sso_proxy_url`. Либо `[web].sso_enabled = false`, либо `sso_proxy_url`
не задан, либо рантайм не поднялся (ищите `sso_config_error` в том же
ответе).

**Тихое обновление токена не срабатывает.** SSO-прокси должен возвращать свежий
токен без полного экрана логина, когда iframe приходит с активной
сессией. У oauth2-proxy это включается флагом `--silent-refresh=true`.

**JWT в cookie игнорируется.** Cookie должна попасть на pg_doorman с
того же домена, и `aud` обязан входить в `sso_audience`. SPA сама
cookie не шлёт; cookie-аутентификация рассчитана на curl, сайдкары и
oauth2-proxy-варианты, которые проставляют токен в cookie на общем
домене.

## Страницы

В SPA доступны:

- **Обзор** — статус сервиса, четыре sparkline по основным сигналам
  (p95 задержки, трафик, ошибок/с, насыщение), stacked area по
  соединениям, heatmap заполнения пулов, двойная ось ожидания и
  возраста самого старого активного запроса, топ-5 ошибок по пулам и
  свёрнутая панель ресурсов.
- **Пулы** — таблица с сортировкой и mini-sparkline в строках.
- **Детали пула** (`/pools/:poolId`) — разбивка по SQLSTATE, возраст
  самого старого активного запроса, кнопки pause / resume / reconnect.
- **Клиенты** — таблица из `/api/clients` с пагинацией, серверной
  фильтрацией и сортировкой.
- **Приложения** — строка на каждый `application_name` с долей ошибок на
  1k запросов.
- **Кеши** — таблица prepared statements с долей попаданий и карточка
  query interner (байты named / anonymous).
- **Логи** — live-tail LogTap с фильтром по level / target и кнопками
  pause / auto-scroll.
- **Конфиг и состояние** — свёрнутые панели: `[general]`, активный фильтр
  логов, кеш auth_query, databases, users, sockets, масштабирование пула,
  координатор пулов.
- **Экран инцидента** (`/wall`) — шесть крупных плиток для incident bridge
  или настенного стенда.

## Сборка из исходников

Собранный фронтенд лежит в git по пути `frontend/dist/`, чтобы
RPM-, DEB- и Docker-пайплайны не зависели от Node.js toolchain.
Разработчикам, правящим SPA, нужно пересобирать его перед коммитом:

```bash
cd frontend
npm ci
npm run install-hooks   # одноразово: ставит pre-commit hook для синхронизации dist
npm run lint
npm run typecheck
npm run build
```

`npm run install-hooks` опционален. CI его не требует: workflow
`.github/workflows/frontend.yml` запускает `npm run check-dist` и
блокирует merge, если исходники меняли без пересборки `dist/`. Тот же
workflow запускает lint и typecheck на каждом PR, который трогает
`frontend/`.

## Развёртывание

`/metrics` доступен без авторизации на том же HTTP-сервере, что и
консоль. Так задумано: иначе сломались бы существующие scrape-конфиги
Prometheus. Авторизация на `/api/*` **не** распространяется на
`/metrics` — метрики раскрывают имена пулов, пользователей и БД,
давление на пул, состояние auth_query и форму нагрузки. Либо держите
секцию `[web]` на приватном host:port, доступном только системе
скрейпа, либо ставьте перед HTTP-сервером прокси, который добавляет
авторизацию на `/metrics` отдельно.
