# Продолжение: Patroni failover discovery

## Промт для новой сессии

```
Продолжаем реализацию Patroni failover discovery.

Ветка: feature/patroni-failover-discovery
План: docs/superpowers/plans/2026-04-24-patroni-failover-discovery.md
Спека: docs/superpowers/specs/2026-04-24-patroni-failover-discovery-design.md
Контекст (результаты ресерча): ~/Projects/pg_doorman_failover.md и ~/Projects/pg_doorman_failover_ru.md

Статус:
- Task 1: DONE — ConnectError + ServerUnavailableError (закоммичено)
- Task 2-10: pending

Режим: subagent-driven-development, model: opus, effort: max
Каждый task: implementer subagent → spec review → code quality review → fix → commit.

Продолжай с Task 2.
```

## Ключевые решения (не терять между сессиями)

1. Модуль `src/patroni/` — HTTP-клиент + types, отдельно от pool
2. `src/pool/failover.rs` — FailoverState (blacklist, whitelist, coalescing)
3. Error classification: `ConnectError` + `ServerUnavailableError` (SQLSTATE 57P), не string parsing
4. Parallel fetch Patroni URLs — fire-and-take-first
5. Parallel TCP connect к members — sync_standby приоритет с 2s grace
6. Сниженный `failover_server_lifetime` для fallback-соединений
7. Request coalescing через `Shared<Future>`
8. Duration config — human parsing ("30s"), не _ms
9. Своя структура Member, не извлекаем из patroni_proxy
10. Reload сбрасывает blacklist + whitelist
11. `patroni_discovery_role` — overengineering, не делаем
12. BDD тесты: inline JSON в feature-файлах, mock Patroni HTTP-сервер
