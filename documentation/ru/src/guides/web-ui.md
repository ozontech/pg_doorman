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
