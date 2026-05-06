# Web UI rollout — handoff в новую сессию

**Состояние на 2026-05-06 19:40 MSK:** PR #236 запушен с 36 коммитами, фазы 1-7 реализованы и положены в master по 5-агентному ревью прошли три fix-коммита. Осталось 5 задач: 4 из ревью + 1 фича по запросу пользователя.

**Branch HEAD:** `acfd50a` на `feat/web-ui`. Дерево чистое.

---

## Что точно работает

- Бинарь pg_doorman на :9127 отдаёт SPA (`/`, `/assets/*`, deep-link fallback на index.html).
- Все 22 read-only API endpoint'а: `/api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/queries`, `/api/top/prepared`, `/api/apps`, `/api/events`.
- Admin-only: `/api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top`. Отдают 401 без `WWW-Authenticate` если `Accept: application/json`, чтобы React-modal не подменялся OS-диалогом браузера.
- Static assets gzip on the fly (Accept-Encoding).
- Default `ui_anonymous = false` — публичные API доступны только под basic-auth.
- LogTap: lock-free Acquire-gate на producer'е, single-task consumer, 30 s reaper.
- 829 lib tests passing, clippy clean, fmt clean. BDD `web-ui.feature` готов.
- Документация `documentation/{en,ru}/src/guides/web-ui.md`.

---

## Осталось сделать (по приоритету)

### 1. PR описание поправить (~5 минут)

Текущее body PR #236 заявляет «refused at startup» про default-password gate. Реально это `log::warn!` + `ui_active = false` (`src/app/server.rs:362-377`). Два варианта:

A. Сделать hard fail: `panic!("admin_password is default — disable [web].ui or set a real password")`.
B. Переписать описание как «warned and disabled, /metrics keeps serving».

Я бы пошёл по B (никто не любит когда pg_doorman ругательно падает после rolling upgrade), но это решение пользователя.

### 2. CI dist sync «исправить жёстко»

`.github/workflows/frontend.yml` сейчас только проверяет что `dist/index.html` существует и в `dist/assets/` есть хоть один JS — это no-op гейт. Если разраб правит `frontend/src/*.tsx` и забывает `npm run build`, в репо едет stale bundle.

Варианты:

- **Pre-commit hook** через `husky`. На `pre-commit` смотрит изменения в `frontend/src/`, запускает `npm run build && git add dist`. Просто в установке (npm install --save-dev husky), но требует `npm install` локально — некоторые контрибьюторы не имеют node toolchain.
- **CI manifest hash check.** Vite пишет `dist/.vite/manifest.json` с хэшами всех assets. CI: пересобрать, сравнить manifest entries (имена + длины) vs committed dist через `diff <(jq -S 'paths' new) <(jq -S 'paths' old)`. Не падает на отличии binary content, ловит изменение source.
- **Расширить existing diff check** — сравнивать gzipped JSON-payload вместо raw JS (gzip более стабилен между Node-версиями). Сомнительно.

Юзер сказал «жёстко» — голосую за husky pre-commit + CI fallback что просто проверяет manifest hash. Делать в `.github/workflows/frontend.yml` и `frontend/package.json` (`husky` + `prepare` script).

### 3. Threshold engine — добавить недостающие правила

`frontend/src/lib/thresholds.ts:122` имеет TODO(phase 6b) для:

- Auth-failure rate (вход: `/api/auth_query`'s `auth_success`/`auth_failure` per database, > 0.5% / > 5% per spec §15.4)
- TLS handshake errors (вход: уже есть `pg_doorman_server_tls_handshake_errors_total` Prometheus, но не в JSON API; либо expose в `/api/pools` либо отдельный `/api/tls`)
- Anonymous LRU evictions (Prometheus `pg_doorman_clients_prepared_anonymous_evictions_total`; добавить в pool DTO?)
- Reconnect rate (`/api/pool_scaling.creates`-delta over time)
- coordinator.exhaustions_total (`/api/pool_coordinator.exhaustions`)
- burst_gate_budget_exhausted (`/api/pool_scaling.gate_budget_ex`)
- fallback_active (это per-pool boolean — есть ли в DTO? проверить)
- Patroni API (если patroni_proxy запущен — есть `/api/patroni`? скорее всего нет, отдельный binary)

Не все доступны через JSON. Минимум — реализовать что можно из существующих DTO; остальное оставить TODO с явным комментарием почему «backend gap».

### 4. SQLSTATE breakdown в Top-5 errors

DBA-ревьюер настаивал. Backend сейчас держит `errors_total: u64` per pool без классификации.

Скоп:
- `src/server/`: tracking `errors_by_sqlstate: HashMap<SmolStr, u64>` per pool (рядом с existing `errors_total`).
- `src/web/routes/dto.rs`: добавить `errors_by_sqlstate: HashMap<String, u64>` в `PoolDto` (только если non-empty, чтобы не раздувать payload).
- Backend already знает SQLSTATE через PostgreSQL ErrorResponse — найти там, где increment'ят `errors_total`, и параллельно классифицировать.
- Frontend `Pools.tsx` drawer: показать breakdown.
- Frontend `Overview.tsx` top-5 errors: optionally split bands per SQLSTATE.

Это самая большая задача. Начинать с отдельного brainstorm.

### 5. Grafana demo фича (требует multi-agent planning)

Юзер явно попросил планирование через 3 ревьюер-агентов параллельно. Задача:

> «В директории grafana лежит demo. Включить webui и настроить пробос необходимых портов. Также туда добавить session mode, если нет.»

Прежде чем начинать кодить — прочитать `grafana/` (что там есть: docker-compose? dashboard JSON? config?), затем запустить три параллельных Agent calls:

- `subagent_type: general-purpose, model: opus` × 3:
  - DevOps: оценить как существующий demo deployment работает, какие porты нужно пробросить для UI (:9127), как сосуществовать с Grafana, нужно ли отдельное `pg_doorman.toml` для demo.
  - DBA: какой сценарий показать в session-mode (отдельный pool с `pool_mode = "session"`); какие подсветки на Grafana dashboard'ах будут полезны.
  - Rust-performance: session mode overhead — нужно ли benchmark'ировать чтобы demo не выдавал false impression.

Свести findings → один writing-plans plan → subagent-driven-development.

---

## Технические заметки для продолжающего

### Где что лежит

```
src/web/
  log_tap.rs              — lock-free LogTap, Acquire gate, single-task consumer, drain() API
  static_assets.rs        — include_dir!, lookup() с SPA fallback
  server.rs               — listener mux, dispatch, ParsedRequest::parse (Accept-аware)
  routes/
    collect/              — 21 файл per-domain после рефакторинга acfd50a
      mod.rs              — общие helpers + pub(crate) use re-exports
      {clients,servers,...}.rs
    dto.rs                — все DTO struct'и (pub(crate))
    {clients,servers,...}.rs — handlers что вызывают collect_*

frontend/
  src/
    api.ts                — fetch wrapper с Authorization: Basic " sentinel против browser cache
    lib/thresholds.ts     — pure threshold engine, TODO(phase 6b) на line 122
    components/
      AuthGate.tsx        — listens unauthorizedAt, modal с Continue anyway / Forget
      HelpTip.tsx         — popover с What/How/Healthy
      MiniSparkline.tsx   — canvas, не uPlot
      ...
    pages/
      Overview.tsx        — Health + 4 sparklines + AreaChart + Heatmap + DualAxis + Top5 + Resource detail
      Pools.tsx           — table с inline mini-sparklines + drawer
      Clients.tsx         — server-side filter/sort/paginate
      Caches.tsx          — Prepared + Query cache tabs
      Logs.tsx            — live tap, level/target filter
      ConfigState.tsx     — 8 tailored panels

documentation/{en,ru}/src/
  guides/web-ui.md        — operator-facing guide

tests/bdd/features/
  web-ui.feature          — 8 BDD сценариев

.github/workflows/
  frontend.yml            — npm ci + lint + typecheck + build + dist exists check
```

### Smoke-команды

```bash
# Запустить pg_doorman:
./target/release/pg_doorman /tmp/doorman-phase5.toml > /tmp/doorman.log 2>&1 &

# Проверить:
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/                           # 200 text/html
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/api/version                # 401 (ui_anonymous=false)
curl -s --user 'admin:phase5test' http://127.0.0.1:9127/api/version                       # 200 + JSON
curl -sI -H 'Accept: application/json' http://127.0.0.1:9127/api/logs | grep -i www-auth  # пусто (silent 401)
curl -sI http://127.0.0.1:9127/api/logs | grep -i www-auth                                # WWW-Authenticate (curl получает challenge)

# Vite dev (если хочешь HMR при правке frontend):
cd frontend && npm run dev -- --host 0.0.0.0   # на 5173/4/5
```

### Известные сюрпризы

- `web::metrics::tests::test_prometheus_server_basic` flaky на hardcoded port 16432. Pre-existing. Re-run проходит. Не надо паниковать.
- Build на CI ubuntu отдаёт другие хэши JS bundle чем local Node 20.18.1 — esbuild native binary issue. Поэтому CI dist sync был ослаблен в `77f715e` до проверки existence. Это и есть TODO #2.
- IBM Plex Sans/Mono коммитятся в `frontend/dist/assets/` для всех subset'ов (cyrillic/latin/...): 60+ woff/woff2 файлов. Можно оптимизировать через subset-specific @fontsource imports — но юзер сказал игнорить такие микрооптимизации.

### Контекстные мемо

- Юзер не любит ceremony на mechanical tasks. Brainstorm/spec — только для creative work (Grafana demo задача — да).
- Юзер просит частые статус-апдейты в Telegram (1-3 предложения, plain text без markdown).
- Push только после явного подтверждения. Commit'ы — авто после plan completion + pre-commit code review (CLAUDE.md правило).
- Commits в репо на английском, комментарии в коде на английском (project convention).
- Memory `feedback_perf_over_accuracy_hot_path`: drop-new и приближённые counters в hot path вместо exact synchronization.
- `feedback_frontend_in_repo_no_ci_build`: dist коммитится, npm в release pipeline нет.

---

## Открытие новой сессии

1. Загрузить эту инструкцию: `Read /home/vadv/Projects/pg_doorman/docs/superpowers/plans/2026-05-06-web-ui-handoff.md`.
2. Загрузить memory `project_web_ui_phase_progress.md` — там же содержание и links на коммиты.
3. Подтвердить с юзером с какого пункта (1-5) продолжать — или попросить новый список приоритетов.
4. Для пунктов 1-2 (PR + CI) — small, можно делать без plan'а.
5. Для 3 (thresholds) — small/medium, но лучше начать с brainstorming чтобы убедиться что endpoints для входов существуют.
6. Для 4 (SQLSTATE) — обязательно brainstorming → spec → plan → subagent-driven.
7. Для 5 (Grafana demo) — обязательно multi-agent параллельный планинг как описано выше.
