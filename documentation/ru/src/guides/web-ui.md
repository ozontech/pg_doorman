# Web UI

pg_doorman поставляется с операторской консолью, которая обслуживается
тем же listener'ом, что и Prometheus-экспортёр. SPA-бандл встроен в
бинарь — то есть деплой остаётся таким же простым: один процесс, один
бинарь, один TCP-порт.

## Включение

UI живёт в секции `[web]`. Старое имя `[prometheus]` принимается как
alias.

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

При `web.ui = true` с пустым или равным `"admin"` `general.admin_password`
сервер тихо понижает консоль до режима «только метрики»: listener
продолжает отдавать `/metrics`, но SPA и admin-эндпоинты выключаются.
Задайте настоящий пароль до того как включать `ui = true`; в логе при
этом появится `web.ui = true ignored: admin_password is default/empty`.

| Параметр | Описание | По умолчанию |
|---|---|---|
| `enabled` | Слушать ли listener вообще. `/metrics` работает независимо от `ui`. | `false` |
| `host` | Bind-адрес. | `"0.0.0.0"` |
| `port` | Bind-порт. | `9127` |
| `ui` | Отдавать SPA по `/` и публичные API-эндпоинты. | `false` |
| `ui_anonymous` | При `true` публичные API (`/api/version`, `/api/overview`, `/api/pools`, ...) принимают запросы без авторизации. Admin-эндпоинты (`/api/logs`, `/api/prepared/text/...`, `/api/interner/top`, `/api/top/queries`, `/api/admin/...`) всегда требуют basic auth. | `false` |
| `log_tap_max_entries` | Размер кольцевого буфера in-memory log tap, обслуживающего `/api/logs`. `0` отключает эндпоинт. | `8192` |

## URL-карта

| URL | Авторизация | Назначение |
|---|---|---|
| `/` и любой non-API путь | Всегда публично, когда `web.ui` активен | SPA-оболочка. Прямой переход на `/pools` не должен дёргать нативный basic-auth диалог браузера до того, как откроется React sign-in модалка — `ui_anonymous` SPA-оболочку не гейтит. |
| `/assets/*` | Всегда публично, когда `web.ui` активен | Хэшированные JS / CSS / шрифты. `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | Без авторизации | Prometheus exposition format. На `ui` не реагирует. |
| `/api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/prepared`, `/api/apps`, `/api/events` | Публично при `ui_anonymous = true`, иначе admin | Read-only JSON. Поля повторяют шейп `SHOW <admin-command>`. |
| `/api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top`, `/api/top/queries` | Admin (basic auth) | Admin-only. `/api/logs` активирует tap при первом запросе и сам выключает его через 2 минуты простоя. `/api/top/queries` возвращает первые ~120 символов SQL-кэша — admin-only, потому что превью могут содержать литералы и идентификаторы клиентов. |

## Авторизация

Консоль использует HTTP basic auth с парой `admin_username` /
`admin_password` из `[general]`. Сравнение пароля — постоянное время.
Браузерам отдаётся `WWW-Authenticate: Basic` при 401, чтобы curl, gh и
сторонние клиенты вели себя нормально. Запросы с
`Accept: application/json` (то, как ходит SPA через `fetch`) получают 401
без challenge — иначе браузер закешировал бы то, что ввёл оператор в
свой OS-диалог, и подменял бы наш React-modal.

По умолчанию креды живут только в React state и теряются при hard
refresh. Чекбокс «Remember me on this device» в sign-in модалке кладёт
их в `localStorage` браузера, чтобы консоль переживала перезагрузку.
Очистка site-storage в браузере удаляет запись.

## Страницы

SPA содержит:

- **Overview** — health-пилл, четыре golden-signals sparkline (latency
  p95, traffic, errors/s, saturation), connection breakdown stacked area,
  pool fill heatmap, dual-axis wait + oldest-active-age, top-5 ошибок по
  пулам и свёрнутый Resource detail.
- **Pools** — sortable таблица с мини-sparkline в строках.
- **Pool detail** (`/pools/:poolId`) — полный drill-down: SQLSTATE-
  разбивка, oldest-active-age, кнопки pause/resume/reconnect.
- **Clients** — paginated таблица из `/api/clients` с server-side filter
  и sort.
- **Apps** — строка на `application_name` с err / 1k q.
- **Caches** — таблица prepared statements с hit rate и карточка query
  interner (named / anonymous bytes).
- **Logs** — live tail LogTap c фильтром по level / target и кнопками
  pause / auto-scroll.
- **Config & state** — свёрнутые панели: `[general]`, активный log filter,
  кэш auth_query, databases, users, sockets, pool scaling, pool coordinator.
- **War room** (`/wall`) — шесть огромных плиток для incident bridge или
  стенда на стене.

## Сборка из исходников

SPA-бандл лежит в git под `frontend/dist/`, чтобы пайплайны
RPM/DEB/Docker не зависели от node toolchain. Разработчикам, правящим
SPA, надо пересобрать перед коммитом:

```bash
cd frontend
npm ci
npm run lint
npm run typecheck
npm run build
```

Отдельный workflow `.github/workflows/frontend.yml` запускает те же
шаги на каждом PR, который трогает `frontend/`.
