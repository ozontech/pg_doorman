# Web UI

В pg_doorman встроена операторская консоль. HTTP-сервер тот же, что отдаёт Prometheus-метрики; собранные файлы фронтенда лежат внутри бинарника. Запуск консоли не добавляет внешних зависимостей: один процесс, один бинарь, один TCP-порт.

## Включение

Консоль настраивается в секции `[web]`. Старое имя секции `[prometheus]` тоже принимается.

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

Если `web.ui = true`, но `general.admin_password` не задан или равен `"admin"`, консоль не запускается. HTTP-сервер продолжает отдавать `/metrics`, но веб-интерфейс и admin-эндпоинты остаются выключенными. В лог пишется `web.ui = true ignored: admin_password is default/empty`. Задайте настоящий пароль до того, как включать `ui = true`.

| Параметр | Описание | По умолчанию |
|---|---|---|
| `enabled` | Запускать ли HTTP-сервер. `/metrics` работает независимо от `ui`. | `false` |
| `host` | Адрес для bind. | `"0.0.0.0"` |
| `port` | Порт для bind. | `9127` |
| `ui` | Отдавать веб-интерфейс по `/` и публичные API-эндпоинты. | `false` |
| `ui_anonymous` | При `true` публичные API (`/api/version`, `/api/overview`, `/api/pools`, ...) принимают запросы без авторизации. Admin-эндпоинты (`/api/logs`, `/api/prepared/text/...`, `/api/interner/top`, `/api/top/queries`, `/api/admin/...`) всегда требуют basic auth. | `false` |
| `log_tap_max_entries` | Размер кольцевого буфера в памяти, обслуживающего `/api/logs`. `0` отключает эндпоинт. | `8192` |

## URL-карта

| URL | Авторизация | Назначение |
|---|---|---|
| `/` и любой не-API путь | Без авторизации, когда `web.ui` активен | Оболочка SPA. Прямой переход на `/pools` открывает форму входа React, а не системный диалог браузера; `ui_anonymous` на доступ к оболочке не влияет. |
| `/assets/*` | Без авторизации, когда `web.ui` активен | Хэшированные JS, CSS и шрифты. `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | Без авторизации | Prometheus exposition format. От `ui` не зависит. |
| `/api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/prepared`, `/api/apps`, `/api/events` | Без авторизации при `ui_anonymous = true`, иначе admin | Read-only JSON. Поля повторяют формат `SHOW <admin-команда>`. |
| `/api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top`, `/api/top/queries` | Admin (basic auth) | Только для admin. `/api/logs` подключает буфер логов при первом запросе и отключает его через 2 минуты простоя. `/api/top/queries` возвращает первые ~120 символов SQL-запросов из кэша; превью могут содержать литералы и идентификаторы клиентов, поэтому admin-only. |

## Авторизация

Консоль использует HTTP basic auth с парой `admin_username` / `admin_password` из секции `[general]`. Пароль сравнивается за постоянное время. На 401 браузерам отдаётся `WWW-Authenticate: Basic`, чтобы `curl`, `gh` и сторонние HTTP-клиенты работали как ожидают. Запросы с заголовком `Accept: application/json` (так SPA ходит через `fetch`) получают 401 без challenge: иначе браузер закешировал бы пароль из системного диалога и подставлял его поверх формы входа React.

По умолчанию реквизиты живут только в памяти React и пропадают при перезагрузке страницы. Если в форме входа отметить «Remember me on this device», реквизиты сохранятся в `localStorage` браузера и переживут перезагрузку. Очистка site storage в браузере удаляет эту запись.

## SSO и роли

Консоль поддерживает три уровня доступа. Они срабатывают на стороне сервера и независят от UI:

| Роль | Активация | Что доступно |
|---|---|---|
| `Anonymous` | нет реквизитов и `ui_anonymous = true` | Публичные `/api/*` без персональных данных. Личные пути (`/api/logs`, `/api/prepared/text/...`, `/api/interner/top`, `/api/top/queries`) и `/api/admin/*` запрещены. |
| `Sso` | валидный JWT в `Authorization: Bearer`, в cookie `sso_access_token=...` или в query `?token=...` | Полный read-only доступ, включая логи и SQL-тексты. Управляющие операции (`POST /api/admin/*`) запрещены — отдаётся `403 Forbidden` с телом `{"error":"forbidden","message":"admin role required"}`. |
| `Admin` | корректный Basic из `[general].admin_username` / `admin_password` | Полный доступ, включая `POST /api/admin/{reload,pause,resume,reconnect}`. |

Когда в одном запросе присутствуют и Basic, и SSO-токен, побеждает Basic — это явный admin-пароль и его нельзя «понизить» SSO-токеном. Если Basic пришёл, но не сошёлся, валидный SSO-токен всё равно даёт `Sso`-роль; логика fallback'а покрывает случай истёкшего токена в `localStorage` рядом с правильным Basic-паролем.

`401 Unauthorized` возвращается, когда реквизитов не было или они были некорректны (классический «надо залогиниться»). `403 Forbidden` — когда реквизиты валидны, но роли не хватает. Фронтенд на 401 поднимает форму входа, на 403 — баннер «admin role required», но не повторно требует логин.

### Включение SSO

1. Возьмите RSA public key, которым SSO proxy подписывает JWT, и положите его в файл (например, `/etc/pg_doorman/sso-public.pem`). Для `oauth2-proxy` ключ извлекается из приватного через `openssl rsa -in private.pem -pubout -out public.pem`. Для Keycloak — из админки realm: Realm Settings → Keys.
2. Добавьте в `pg_doorman.toml` секцию `[web]` с SSO-полями:

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

3. Перезагрузите конфиг: `kill -SIGHUP <pid>` или `psql -h <host> -p 6432 -U admin -d pgbouncer -c 'RELOAD'`.
4. Проверьте: `curl http://<host>:9127/api/auth/config` должен вернуть `"sso_enabled":true` и `"sso_proxy_url":"..."`.

| Поле | Назначение | По умолчанию |
|---|---|---|
| `sso_enabled` | Включает SSO-ветку. Без неё JWT не валидируются. | `false` |
| `sso_proxy_url` | URL внешнего SSO proxy. Используется только фронтендом для редиректа на «Sign in via SSO». Серверная валидация на это поле не смотрит. | `null` |
| `sso_public_key_file` | Путь к PEM-файлу с RSA public key. Читается на старте и при `RELOAD`. | `null` |
| `sso_audience` | Список допустимых значений claim `aud`. Токен валиден, если хотя бы одно совпадает. Обязательное поле при `sso_enabled = true`. | `[]` |
| `sso_allowed_users` | Allowlist по `preferred_username` или `sub`. `["*"]` принимает любого. Иначе только перечисленные. | `["*"]` |

Если `sso_enabled = true`, но `sso_public_key_file` не задан или PEM не читается, в лог пишется `error` и SSO молча отключается на этот запуск — листенер продолжает работать только на Basic. Это поведение защищает консоль от падения из-за опечатки в SSO-секции.

### Логин из браузера

При первом заходе пользователь попадает на форму входа. Если в `/api/auth/config` указан `sso_proxy_url`, в форме появляется кнопка **Sign in via SSO**. Клик отправляет браузер на `sso_proxy_url?redirect_to=<текущий URL>`. Внешний proxy выполняет OAuth/OIDC-флоу и возвращает пользователя обратно с `?token=<jwt>`. SPA захватывает этот токен в `localStorage`, чистит URL и работает дальше.

В правом нижнем углу sidebar отображается имя текущего пользователя: `admin` для Basic или `sso: <preferred_username>` для SSO. Кнопка sign out очищает оба ключа `localStorage` (`pgdoorman.admin-auth` и `pgdoorman.sso-token`) и снова открывает форму входа.

Тихое обновление токена — раз в 60 секунд, за 90 секунд до истечения срока. Срабатывает скрытым iframe, который приземляется на `?sso_silent=1`. На этой страничке App рендерит минимальный SilentCallback вместо обычного UI и через `postMessage` возвращает новый токен в parent-окно. На случай отказа silent refresh: если у пользователя есть Basic, токен молча сбрасывается; иначе — полный редирект на SSO proxy. Минимальная рекомендованная длительность жизни JWT — 5 минут.

### Access-лог

После каждого HTTP-ответа консоль пишет одну строку logfmt в стандартный логгер pg_doorman:

```
INFO pg_doorman::web::access method=GET path=/api/admin/reload query=false status=200 bytes=42 latency_ms=12 peer=10.0.1.5:42312 auth_role=admin auth_source=basic auth_user=admin
```

Поля: `method`, `path`, `query=true|false`, `status`, `bytes`, `latency_ms`, `peer` (адрес клиента, если pg_doorman стоит за reverse proxy — это адрес proxy), `auth_role` (`admin`/`sso`/`anonymous`/`rejected`), `auth_source` (`basic`/`sso`/`-`), `auth_user`. Тело запроса/ответа и query string в лог не пишутся. Цель `pg_doorman::web::access` — отдельная, чтобы фильтровать в `/api/logs` через target-фильтр LogTap.

### Troubleshooting

- **401 на валидном JWT**. Проверьте, что `aud` в токене попадает в `sso_audience` и `exp` ещё не истёк. PEM можно проверить через `openssl rsa -pubin -in <pem> -text -noout`.
- **403 на запросе с валидным JWT**. Это путь, требующий `Admin` (например, `POST /api/admin/reload`). SSO даёт только read-only доступ.
- **Silent refresh не срабатывает**. Проверьте, что oauth2-proxy не редиректит на полный экран логина, когда iframe приходит без активной сессии. У oauth2-proxy за это отвечает `--silent-refresh = true`.
- **JWT приходит в cookie, но не валидируется**. Cookie ставится на тот же домен, что и pg_doorman? `aud` совпадает с `sso_audience`?

## Страницы

В SPA доступны:

- **Overview** — индикатор health, четыре sparkline по golden signals (latency p95, traffic, errors/s, saturation), stacked area по соединениям, heatmap заполнения пулов, двойная ось wait + oldest-active-age, топ-5 ошибок по пулам и свёрнутая панель Resource detail.
- **Pools** — таблица с сортировкой и mini-sparkline в строках.
- **Pool detail** (`/pools/:poolId`) — детальный разбор: разбивка по SQLSTATE, oldest-active-age, кнопки pause / resume / reconnect.
- **Clients** — таблица из `/api/clients` с пагинацией, серверной фильтрацией и сортировкой.
- **Apps** — строка на каждый `application_name` с долей ошибок на 1k запросов.
- **Caches** — таблица Prepared Statements с hit rate и карточка query interner (named / anonymous bytes).
- **Logs** — live-tail LogTap с фильтром по level / target и кнопками pause / auto-scroll.
- **Config & state** — свёрнутые панели: `[general]`, активный фильтр логов, кэш auth_query, databases, users, sockets, pool scaling, pool coordinator.
- **War room** (`/wall`) — шесть крупных плиток для incident bridge или стенда на стене.

## Сборка из исходников

Собранный фронтенд лежит в git по пути `frontend/dist/`, чтобы пайплайны RPM, DEB и Docker не зависели от node toolchain. Разработчикам, правящим фронтенд, нужно пересобирать его перед коммитом:

```bash
cd frontend
npm ci
npm run install-hooks   # одноразово: ставит pre-commit hook для синхронизации dist
npm run lint
npm run typecheck
npm run build
```

`npm run install-hooks` опционален. CI его не требует: workflow `frontend.yml` запускает `npm run check-dist` и блокирует merge, если исходники меняли без пересборки `dist/`.

Отдельный workflow `.github/workflows/frontend.yml` запускает те же шаги на каждом PR, который трогает `frontend/`.

## Развёртывание

`/metrics` доступен без авторизации на том же HTTP-сервере, что и консоль. Так задумано: иначе сломались бы существующие scrape-конфиги Prometheus. Если pg_doorman стоит за reverse proxy с авторизацией на `/api/*`, эта авторизация **не** распространяется на `/metrics`. Метрики раскрывают имена пулов, пользователей и БД, давление на пул, состояние auth_query и форму нагрузки. Поэтому либо держите секцию `[web]` на приватном host:port, доступном только системе скрейпа, либо ставьте перед HTTP-сервером proxy, который добавляет авторизацию на `/metrics` отдельно.
