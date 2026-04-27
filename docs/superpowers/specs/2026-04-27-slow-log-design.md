# Slow log: дизайн

**Дата:** 2026-04-27
**Статус:** Спека готова, имплементация не начата
**Scope:** Phase 1 (минимум жизнеспособного slow log)

## 1. Контекст и мотивация

PostgreSQL уже пишет медленные запросы через `log_min_duration_statement` и `pg_stat_statements`. Дублировать это в pg_doorman бессмысленно. Уникальная ценность slow log в пулере — **объяснить разрыв между «клиент видит медленно» и «PG говорит запрос быстрый»**: куда в этом окне ушло время.

### Литмус-тест каждого поля
> Может ли DBA узнать это из `log_min_duration_statement` или `pg_stat_statements`? Если да — не логируем. Если нет — наш домен.

Под этот тест попадают: время ожидания backend connection, время удержания транзакции idle-клиентом, объёмы трафика клиент↔пулер.

## 2. Что собираем

### Тайминги

| Поле | Способ замера | Стоимость |
|------|---------------|-----------|
| `wait_us` | `max` по `checkout_us` за xact (`transaction.rs:771`) | 0 — уже считается |
| `xact_us` | `session_xact_start.elapsed()` при complete | 0 — уже считается |
| `idle_us` | сумма gap'ов между `Sync`/`Flush`/`CopyDone` и следующим клиентским сообщением внутри xact'а | **+1 `now()`** на завершение query |
| `exec_us` (derived) | `xact_us - wait_us - idle_us` через `saturating_sub` | 0 |

При выключенном slow log (все пороги = 0) дополнительные `now()` не вызываются, hot path — один atomic load + branch.

### Volume

`bytes_c2s` / `bytes_s2c` — **approximate** дельта `address.bytes_received/sent` снятая на `begin_xact` и на `maybe_emit`. Per-pool счётчик загрязнён трафиком других клиентов того же пула, но порядок величины достоверен — этого достаточно для оператора.

## 3. Reason codes и hints

Один primary reason на запись. Множественность сработавших порогов видна по числам в строке (`wait=`, `idle=`).

| Code | Когда выбирается primary | Hint (`&'static str`) |
|------|--------------------------|----------------------|
| `WAIT_QUEUE` | `wait_us >= max(idle_us, exec_us)` | `"pool exhausted, all backends busy"` |
| `IDLE_IN_TX` | `idle_us > max(wait_us, exec_us)` | `"client held tx idle between statements"` |
| `LONG_XACT` | иначе | `"long active transaction"` |

При обрыве соединения с открытой транзакцией используется **та же логика выбора primary** — фактическая запись эмитится только если хотя бы один порог сработал к моменту обрыва. Отдельного aborted-кода в Phase 1 нет.

## 4. Формат записи

Logfmt, согласованный со стилем `src/stats/print_all_stats.rs`. Уровень — `warn!()`, в общий лог. Без отдельного target, без отдельного файла, без admin SHOW.

### Шаблон

```
[<user>@<db> #c<id>] slow xact reason=<CODE> hint="<phrase>" pid=<pid> mode=<txn|sess> xact_id=<n> queries=<n> wait=<N>ms xact=<N>ms idle=<N>ms bytes=<n>/<n>
```

### Примеры

```
[app@billing #c1234] slow xact reason=WAIT_QUEUE hint="pool exhausted, all backends busy" pid=2891 mode=transaction xact_id=15 queries=1 wait=1750ms xact=1820ms idle=0ms bytes=86/127

[app@billing #c1234] slow xact reason=IDLE_IN_TX hint="client held tx idle between statements" pid=2891 mode=transaction xact_id=16 queries=1 wait=0ms xact=4012ms idle=3998ms bytes=148/89

[etl@batch #c5678] slow xact reason=LONG_XACT hint="long active transaction" pid=2912 mode=transaction xact_id=3 queries=51 wait=12ms xact=3084ms idle=4ms bytes=14823/892
```

`reason` и `hint` — `&'static str`, нулевая аллокация в форматтере.

## 5. Архитектура

### Расположение
- Новый файл: `src/client/slow_log.rs`
- Новая секция конфига: `src/config/slow_log.rs`
- Изменения в `src/client/core.rs`, `src/client/transaction.rs`

### Тип `SlowLogTracker`
Per-client accumulator, ~56 байт, без heap-аллокаций.

```rust
#[derive(Default)]
pub(crate) struct SlowLogTracker {
    enabled: bool,                            // snapshot на начале xact
    queries: u32,
    max_wait_us: u64,
    idle_us: u64,
    bytes_c2s_at_begin: u64,
    bytes_s2c_at_begin: u64,
    last_query_end: Option<quanta::Instant>,
}
```

### API
Все методы `#[inline]`, ранний `return` при `enabled = false`.

| Метод | Когда вызывается |
|-------|------------------|
| `begin_xact(bytes_c2s_baseline, bytes_s2c_baseline)` | старт транзакции, рядом с `session_xact_start = Some(now())` |
| `observe_wait(wait_us)` | сразу после `checkout_us` (`transaction.rs:771`), max-агрегация |
| `observe_query_end(now)` | конец batch'а: `handle_simple_query`, `handle_sync_flush`, `handle_copy_done_fail` |
| `observe_message_start(now)` | в transaction loop при получении следующего message от клиента |
| `maybe_emit(ctx, xact_us, bytes_c2s_now, bytes_s2c_now)` | в `complete_transaction_if_needed` после `session_xact_start = None` |
| `maybe_emit_aborted(ctx, xact_us, ...)` | при Terminate/disconnect внутри открытой xact |

### Глобальное runtime-состояние

```rust
pub(crate) static SLOW_LOG_WAIT_US: AtomicU64 = AtomicU64::new(0);
pub(crate) static SLOW_LOG_IDLE_US: AtomicU64 = AtomicU64::new(0);
pub(crate) static SLOW_LOG_XACT_US: AtomicU64 = AtomicU64::new(0);
pub(crate) static SLOW_LOG_ENABLED: AtomicBool = AtomicBool::new(false);
```

`SLOW_LOG_ENABLED` = OR трёх порогов > 0. Один branch в hot path.

## 6. Конфигурация

### TOML

```toml
[slow_log]
wait_threshold_ms = 0    # 0 = выключено для этого порога
idle_threshold_ms = 0
xact_threshold_ms = 0
```

Все три по умолчанию 0. Любая комбинация валидна. Единицы — миллисекунды (как у `pooler_check_query_idle_timeout_ms` и др. в существующем конфиге).

### Reload

При SIGHUP / `RELOAD` обновляются три атомика (конверсия `ms → µs` происходит один раз при reload, дальше hot path сравнивает µs с µs):

```rust
SLOW_LOG_WAIT_US.store(cfg.wait_threshold_ms.saturating_mul(1000), Relaxed);
SLOW_LOG_IDLE_US.store(cfg.idle_threshold_ms.saturating_mul(1000), Relaxed);
SLOW_LOG_XACT_US.store(cfg.xact_threshold_ms.saturating_mul(1000), Relaxed);
SLOW_LOG_ENABLED.store(
    cfg.wait_threshold_ms | cfg.idle_threshold_ms | cfg.xact_threshold_ms > 0,
    Relaxed
);
```

Открытые транзакции дочитываются по старым порогам (snapshot уже сделан в `begin_xact`). Новые транзакции видят новые пороги с первой же `begin_xact`.

### `SHOW CONFIG`
Три новых строки автоматически попадают в существующий механизм отображения config'а.

## 7. Логика `maybe_emit`

```rust
pub fn maybe_emit(&mut self, ctx: SlowLogCtx<'_>, xact_us: u64, bytes_c2s_now: u64, bytes_s2c_now: u64) {
    let wait_us = self.max_wait_us;
    let idle_us = self.idle_us;

    let wait_t = SLOW_LOG_WAIT_US.load(Relaxed);
    let idle_t = SLOW_LOG_IDLE_US.load(Relaxed);
    let xact_t = SLOW_LOG_XACT_US.load(Relaxed);

    let wait_hit = wait_t > 0 && wait_us >= wait_t;
    let idle_hit = idle_t > 0 && idle_us >= idle_t;
    let xact_hit = xact_t > 0 && xact_us >= xact_t;

    if !(wait_hit || idle_hit || xact_hit) {
        self.reset();
        return;
    }

    let exec_us = xact_us.saturating_sub(wait_us).saturating_sub(idle_us);

    let (reason, hint) = if wait_us >= idle_us && wait_us >= exec_us {
        ("WAIT_QUEUE", HINT_WAIT_QUEUE)
    } else if idle_us > exec_us {
        ("IDLE_IN_TX", HINT_IDLE_IN_TX)
    } else {
        ("LONG_XACT", HINT_LONG_XACT)
    };

    let bytes_c2s = bytes_c2s_now.saturating_sub(self.bytes_c2s_at_begin);
    let bytes_s2c = bytes_s2c_now.saturating_sub(self.bytes_s2c_at_begin);

    warn!(
        "[{}@{} #c{}] slow xact reason={} hint=\"{}\" pid={} mode={} xact_id={} \
         queries={} wait={}ms xact={}ms idle={}ms bytes={}/{}",
        ctx.user, ctx.db, ctx.client_id, reason, hint, ctx.server_pid, ctx.mode_str,
        ctx.xact_id, self.queries, wait_us / 1_000, xact_us / 1_000, idle_us / 1_000,
        bytes_c2s, bytes_s2c,
    );

    self.reset();
}
```

При ничьей `wait_us == idle_us == exec_us` (или две из трёх равны и доминируют) — приоритет `WAIT_QUEUE`. Это намеренно: начало транзакции — самая операционно-болезненная фаза.

## 8. Поддержка extended protocol

`observe_query_end` вызывается в трёх точках:

| Сценарий | Callsite |
|----------|----------|
| Simple `Q` | `handle_simple_query` после `execute_server_roundtrip` |
| Extended `Sync`/`Flush` | `handle_sync_flush` после `execute_server_roundtrip` для обоих кодов |
| `CopyDone`/`CopyFail` | `handle_copy_done_fail` после `write_all_flush` |

`Parse`/`Bind`/`Describe`/`Execute`/`Close` сами по себе **не** вызывают `observe_query_end` — они буферизуются, ответ клиенту приходит только при `Sync`/`Flush`. Один pipelined batch с одним `Sync` = один `queries++`. Async client (Flush-driven) инкрементирует на каждый `Flush` и финальный `Sync` — это согласуется с семантикой «клиент дождался ответа».

`observe_message_start` ставится в transaction loop перед обработкой кода входящего сообщения (`transaction.rs:893-933`).

## 9. Edge cases

| Сценарий | Поведение |
|----------|-----------|
| Standalone BEGIN с deferred dispatch (`transaction.rs:689-707`) | `begin_xact` срабатывает только когда реальная xact стартует на сервере (вместе с `session_xact_start = Some(now())`). До этого — клиент не «в транзакции» с точки зрения pooler'а. |
| Implicit autocommit (simple Q без BEGIN) | `begin_xact` в `transaction.rs:1085-1086` где уже выставляется `session_xact_start`. queries=1, idle=0, xact_us ≈ query_us. |
| `pooler_check_query` | Перехватывается в `try_handle_without_server`, не идёт через xact lifecycle — slow log его не видит. |
| DEALLOCATE intercepted | Та же логика — без сервера, не часть xact'а. |
| Cancel request | Отдельный сокет, `handle_cancel_mode`, к tracker'у не относится. |
| Counter wraparound (`bytes_now < bytes_at_begin`) | `saturating_sub` даёт 0. |
| Реload во время xact | Снапшот порогов в `begin_xact` — открытые xact'ы дочитываются по старым значениям, новые видят новые. |

## 10. Тестирование

### Unit-тесты `SlowLogTracker`

| Тест | Что проверяет |
|------|---------------|
| `begin_xact_resets_state` | ненулевые поля → `begin_xact` → всё ноль |
| `observe_wait_takes_max` | три вызова с разными значениями → `max_wait_us` максимум |
| `idle_accumulates_across_messages` | две пары (query_end → gap → message_start) → суммарный `idle_us` |
| `primary_wait_when_dominant` | wait>idle, wait>exec → `WAIT_QUEUE` |
| `primary_idle_when_dominant` | idle>wait, idle>exec → `IDLE_IN_TX` |
| `primary_long_xact_default` | exec доминирует → `LONG_XACT` |
| `primary_wait_wins_tie` | wait==idle → `WAIT_QUEUE` (правило приоритета начала xact) |
| `disabled_skips_all_work` | все пороги=0 → emit ничего не пишет (через mock логгер) |
| `bytes_delta_saturates` | bytes_now < bytes_at_begin → `saturating_sub` = 0 |

### BDD

Базовый набор сценариев в `tests/bdd/features/slow-log.feature`:
- `WAIT_QUEUE` появляется при исчерпанном пуле
- `IDLE_IN_TX` появляется при медленном клиенте внутри xact
- `LONG_XACT` появляется при долгом батче
- Все пороги=0 — никаких записей в логе
- Reload меняет пороги без перезапуска

Полный список сценариев и edge cases — отдельная итерация (например, async pipelining с Flush, COPY in xact, multi-statement xact с разной типизацией доминирующего времени) — будет проработан при имплементации.

## 11. Что НЕ входит в Phase 1

Сознательно отрезано — добавляется отдельными итерациями при необходимости:

- **Полный текст запроса** — даже за флагом, требует осторожного дизайна (PII, log volume, prepared statement name vs текст).
- **Query digest** — xxh3 hash от Parse-сообщения как correlation с `pg_stat_statements.queryid`. Полезно, но требует хранения и нормализации.
- **`server_acquire_us`** — деталировка фазы между checkout и первым байтом серверу (включает `sync_server_parameters` и deferred BEGIN). На unix-socket setup'ах с выключенным `sync_server_parameters` — близко к нулю.
- **`cleanup_us`** — время `checkin_cleanup` (DEALLOCATE/RESET). Часть `wait_us` для следующего клиента, не для текущего.
- **Per-client точные `bytes`** — требует новых полей в `ClientStats`, фаза 1 живёт с approximate per-pool дельтой.
- **`SHOW SLOW_LOG` в admin** — отвергнуто по запросу: общего лога достаточно для дебага.
- **Отдельный log target / отдельный файл** — отвергнуто: один `warn!()` в общий лог.
- **Per-pool / per-user пороги** — пока глобальные. Если потребуется — добавим override на уровне `[user.<name>]` / `[pool.<name>]`.
- **Heartbeat для висящих транзакций** — если клиент висит вечно без commit/disconnect, запись не появится. Видимость через `SHOW CLIENTS` / `SHOW SERVERS` и через `maybe_emit_aborted` при eventual disconnect.

## 12. Ключевые ссылки на код

- Existing timing points: `src/client/transaction.rs:677, 715, 805, 881, 939`
- `session_xact_start` lifecycle: `src/client/transaction.rs:174-186, 1085-1086`
- Bytes counters: `src/stats/address.rs:222-242` (`bytes_received_add`, `bytes_sent_add`)
- Logfmt reference: `src/stats/print_all_stats.rs:19-51`
- Pooler check / DEALLOCATE intercept: `src/client/transaction.rs` `try_handle_without_server`
- Reload entrypoint: `src/admin/commands.rs:16-36` (`RELOAD`), сигнал SIGHUP — `src/app/server.rs`
