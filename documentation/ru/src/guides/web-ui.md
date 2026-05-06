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
ui_anonymous = true
log_tap_max_entries = 8192
```

При `web.ui = true` сервер откажется стартовать, если `general.admin_password`
пустой или равен `"admin"` — иначе все admin-эндпоинты остались бы
тривиально открытыми. Задайте настоящий пароль до того как включать
`ui = true`.

| Параметр | Описание | По умолчанию |
|---|---|---|
| `enabled` | Слушать ли listener вообще. `/metrics` работает независимо от `ui`. | `false` |
| `host` | Bind-адрес. | `"0.0.0.0"` |
| `port` | Bind-порт. | `9127` |
| `ui` | Отдавать SPA по `/` и публичные API-эндпоинты. | `false` |
| `ui_anonymous` | При `true` публичные API (`/api/version`, `/api/overview`, `/api/pools`, ...) принимают запросы без авторизации. Admin-эндпоинты (`/api/logs`, `/api/prepared/text/...`, `/api/interner/top`) всегда требуют basic auth. | `true` |
| `log_tap_max_entries` | Размер кольцевого буфера in-memory log tap, обслуживающего `/api/logs`. `0` отключает эндпоинт. | `8192` |

## URL-карта

| URL | Авторизация | Назначение |
|---|---|---|
| `/` и любой non-API путь | Публично при `ui_anonymous = true`, иначе basic-auth | SPA-оболочка. Маршрутизация клиентская, поэтому deep-link `/pools` выживает hard refresh. |
| `/assets/*` | Как и `/` | Хэшированные JS / CSS / шрифты. `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | Без авторизации | Prometheus exposition format. На `ui` не реагирует. |
| `/api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/queries`, `/api/top/prepared`, `/api/apps`, `/api/events` | Публично при `ui_anonymous = true`, иначе admin | Read-only JSON. Поля повторяют шейп `SHOW <admin-command>`. |
| `/api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top` | Admin (basic auth) | Admin-only. `/api/logs` активирует tap при первом запросе и сам выключает его через 30 с простоя. |

## Авторизация

Консоль использует HTTP basic auth с парой `admin_username` /
`admin_password` из `[general]`. Сравнение пароля — постоянное время.
Браузерам отдаётся `WWW-Authenticate: Basic` при 401, чтобы curl, gh и
сторонние клиенты вели себя нормально. Запросы с
`Accept: application/json` (то, как ходит SPA через `fetch`) получают 401
без challenge — иначе браузер закешировал бы то, что ввёл оператор в
свой OS-диалог, и подменял бы наш React-modal.

Креды, введённые в консоль, живут только в React state и теряются при
перезагрузке вкладки.

## Страницы

SPA содержит шесть страниц:

- **Overview** — health-пилл, четыре golden-signals sparkline (latency
  p95, traffic, errors/s, saturation), connection breakdown stacked area,
  pool fill heatmap, dual-axis wait + oldest-active-age, top-5 ошибок по
  пулам и свёрнутый Resource detail.
- **Pools** — sortable таблица с мини-sparkline в строках и drawer с
  деталями пула по клику.
- **Clients** — paginated таблица из `/api/clients` с server-side filter
  и sort.
- **Caches** — таблица prepared statements с hit rate и карточка query
  interner (named / anonymous bytes).
- **Logs** — live tail LogTap c фильтром по level / target и кнопками
  pause / auto-scroll.
- **Config** — восемь свёрнутых панелей: `[general]`, активный log filter,
  кэш auth_query, databases, users, sockets, pool scaling, pool coordinator.

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
