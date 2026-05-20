# Инцидент 2026-05-20: EMFILE при бинарном upgrade 3.6.5 → 3.9.1

Постмортем production-инцидента. Описывает наблюдаемое поведение, корневые причины и план починки по приоритетам. Код в этом коммите не меняется — для каждого пункта плана будет отдельный PR.

## TL;DR

При накатывании 3.9.1 поверх работающего 3.6.5 на ~20 машинах из 20 тысяч pg_doorman вошёл в состояние Too-many-open-files и не вернулся к нормальной работе до ручного restart. Корневые причины:

1. Миграционный буфер `MIGRATION_CHANNEL_CAPACITY=4096` (`src/app/server.rs:77`) не привязан к текущему `RLIMIT_NOFILE`. Каждый клиент в очереди миграции — это второй fd в parent (через `libc::dup` в `prepare_migration`, `src/client/migration.rs:177`). При 3.2K клиентов и `LimitNOFILE=6200` это превышает лимит.
2. EMFILE на `connect()` к backend классифицируется как «primary unreachable» (`src/pool/server_pool.rs:1379`), запускает Patroni-фолбэк, который сам падает с EMFILE.
3. Когда EMFILE-цикл начался, выйти из него нечем: blacklist re-armed на каждой итерации (`src/pool/fallback.rs:229`), клиенты не отбрасываются, fd не освобождаются. Процесс продолжает выдавать ошибки до ручного restart.

План починки. Главное (приоритет P0) — привязать миграционный буфер к живому nofile-budget вместо константы 4096, и добавить выход из затяжного EMFILE через отбрасывание клиентов. Менее срочное (P1) — отличать локальную ресурсную нехватку от сетевой недоступности на backend connect, чтобы EMFILE не уходил в Patroni-фолбэк.

Менять `LimitNOFILE` снаружи, делать `setrlimit` в pg_doorman или иначе влиять на заданный оператором лимит не предлагается. Любой fix остаётся внутри границ лимита.

## Контекст инцидента

* Версия до: 3.6.5. Версия после: 3.9.1.
* Маршрут: 3.9.1 → 3.6.4 (downgrade прошёл штатно) → 3.6.4 → 3.9.1 (upgrade завершился инцидентом). Тега `v3.6.4` в репо нет — это внутренняя сборка между релизами, для анализа diff'а считаем эквивалентом v3.6.5.
* Профиль нагрузки на пострадавшей машине: 3200 client TCP-соединений, 10 backend unix-socket к PG, transaction pooling, нет dynamic pools, TLS включён, Patroni-fallback включён с URL `http://localhost:8008`.
* `LimitNOFILE=6200` в `[Service]` секции unit-файла под runr (`/etc/runr/pg_doorman.service`).
* Полечилось рестартом pg_doorman. Автоматически не восстановилось.

Видимая ошибка в логах:

```
Could not get a database connection from the pool. All servers may be busy
or down. Error details: Error occurred while creating a new connection to
postgresql (postgresql_login_error): Backend connect error: fallback
discovery failed: all patroni urls failed: http://localhost:8008: error
sending request for url (http://localhost:8008/cluster): error trying to
connect: dns error: Too many open files (os error 24). Please try again
later. (SQLSTATE 53300)
```

Сообщение про Patroni — производное. Корневой EMFILE возник раньше, при connect к unix-socket backend; в Patroni-фолбэк попали из-за неправильной классификации этой ошибки.

## Системный фон

Под runr cgroup-ограничители из конфига применяются только для `memory.max`, `cpu.max`, `io.max` (см. `src/cgroup/apply.rs` в репо `~/Projects/runr`). Контроллера на file descriptors в cgroup v2 нет. `LimitNOFILE=6200` применяется через `setrlimit(RLIMIT_NOFILE)` в `pre_exec` дочернего процесса (`src/orchestration/process/runner_unix.rs:355-381`). Это per-process POSIX-лимит. При fork+exec child наследует RLIMIT_NOFILE от parent через `task_struct` — runr дополнительный setrlimit для child не применяет.

С точки зрения kernel два процесса pg_doorman (parent + child) во время handoff могут параллельно держать каждый свои 6200 fd, суммарно 12400. Общего каунтинга по cgroup нет. Системные глобальные лимиты (`/proc/sys/fs/file-max`, `nr_open`) на порядки выше per-process limit и не задействованы.

Значит, EMFILE в инциденте возник в **одном** процессе, который сам по себе перешагнул свои 6200 fd. Какой именно процесс и почему — далее.

## Наблюдаемое поведение

В момент пика handoff'а (когда parent поэтапно мигрирует клиентов в child через `sendmsg(SCM_RIGHTS)`) parent process удерживает в fd-таблице:

| Источник | Кол-во fd |
|---|---|
| Живые client TCP (ещё не мигрировавшие) | до 3200 |
| dup'нутые fd, ожидающие sendmsg | до 3200 |
| backend unix-socket | 10 |
| Web UI listener | 1 |
| migration socketpair, parent end | 1 |
| readiness pipe, parent end | 1 |
| tokio I/O driver и signalfd | 2-5 |
| stdin/stdout/stderr | 3 |
| **Итого peak** | около 6420 fd |

6420 > 6200, и parent попадает в EMFILE. Дальнейшие `connect()`, `dup()`, `accept()` и любая операция, требующая нового fd, падают.

После EMFILE в логе появляется ошибка на каждом client request (`log::error!` в `src/server/stream.rs:149` и `:176` без rate-limit), и поток зацикливается в Patroni-фолбэк (см. ниже). Этот режим держится пока fd не освободятся — а изнутри процесса fd не освобождаются ничем. Оператор закончил инцидент ручным `systemctl restart pg_doorman`.

## Причина 1: миграционный буфер не учитывает nofile budget

Архитектура миграции при SIGUSR2 такая. Parent делает `fork+exec` нового бинаря (`src/app/server.rs:1016-1037`) с переданным через `--inherit-fd` PG-listener'ом и `PG_DOORMAN_MIGRATION_FD` / `PG_DOORMAN_READY_FD` в env. После ready-сигнала parent освобождает PG listener (`*listener = None`, `src/app/server.rs:1082`) и начинает приём клиентов в очередь миграции.

Каждый client в parent в idle-точке handle-loop (`src/client/transaction.rs:680-737`) вызывает `prepare_migration()`. Эта функция делает `libc::dup(raw_fd)` (`src/client/migration.rs:177`):

```rust
let dup_fd = unsafe { libc::dup(raw_fd) };
if dup_fd < 0 {
    return Err(Error::SocketError(
        "dup() failed during migration".to_string(),
    ));
}
```

Результат: на каждого клиента parent временно держит **два fd** — оригинальный (живой в client task до `return Ok(())`) и dup'нутый (в `MigrationPayload`, ждёт пока sender_task отправит через sendmsg). После успешного sendmsg parent делает `libc::close(payload.fd)` (`src/client/migration.rs:752`), refcount возвращается к 1.

`MIGRATION_TX` имеет capacity 4096:

```rust
// src/app/server.rs:77
const MIGRATION_CHANNEL_CAPACITY: usize = 4096;
```

При нагрузочном профиле пользователя (3200 клиентов) очередь успевает заполниться раньше, чем sender_task её разгребёт. Каждый из 3200 ждёт в очереди со своим dup'нутым fd. В пике это и есть те самые ~3200 дополнительных fd в parent, которые поверх живых клиентов и backends упираются в 6200.

Корень проблемы — константа 4096 без оглядки на текущий ulimit. Если бы capacity вычислялось как функция от `RLIMIT_NOFILE`, ожидаемого числа клиентов и пула, на 6200-лимите capacity получалась бы порядка 2000-3000 fd, и parent не превышал бы свой fd-budget.

## Причина 2: EMFILE классифицируется как primary unreachable

Backend connect к unix-socket (`src/server/stream.rs:144-154`):

```rust
pub(crate) async fn create_unix_stream_inner(host: &str, port: u16) -> Result<StreamInner, Error> {
    let started = Instant::now();
    let stream = match UnixStream::connect(&format!("{host}/.s.PGSQL.{port}")).await {
        Ok(s) => s,
        Err(err) => {
            log::error!("Failed to connect to Unix socket {host}:{port}: {err}");
            return Err(Error::ConnectError(format!(
                "Failed to connect to Unix socket {host}:{port}: {err}"
            )));
        }
    };
    ...
}
```

Аналогично для TCP (`src/server/stream.rs:166-180`). Любой `io::Error`, включая EMFILE, мапится в `Error::ConnectError(String)` — категория, не различающая локальное исчерпание ресурса и сетевую недоступность peer.

В pool:

```rust
// src/pool/server_pool.rs:1379-1384
fn is_backend_unreachable(err: &Error) -> bool {
    matches!(
        err,
        Error::ConnectError(_) | Error::ServerUnavailableError(_, _)
    )
}
```

И триггер фолбэка:

```rust
// src/pool/server_pool.rs:523-538
if is_backend_unreachable(&err) {
    if let Some(ref fallback) = self.fallback_state {
        fallback.blacklist();
        ...
        return self.create_fallback_connection().await;
    }
}
```

EMFILE при `UnixStream::connect` идёт сюда как `ConnectError`. `is_backend_unreachable` возвращает true, и код переключается на Patroni-фолбэк-путь. Patroni HTTP-запрос через `reqwest` тоже требует новый socket и тоже падает с EMFILE — это и есть строка `fallback discovery failed: ...`, которая видна в логах. Корневой EMFILE при connect к PG в явном виде в лог не попадает.

Не фатально (миграция уже сломана по другой причине), но создаёт ложный сигнал в логах: оператор смотрит на ошибки Patroni-discovery и думает что Patroni сломан, хотя на самом деле сломан local fd budget.

## Причина 3: нет автоматического выхода из затяжного EMFILE

`FallbackState::blacklist()` (`src/pool/fallback.rs:229-232`):

```rust
pub fn blacklist(&self) {
    let mut guard = self.blacklisted_until.lock();
    *guard = Some(Instant::now() + self.blacklist_duration);
}
```

`blacklist_duration` — это `fallback_cooldown` из конфига, default 30 секунд (`src/pool/mod.rs:1131`).

`check_blacklist` (`src/pool/fallback.rs:197-227`) после истечения возвращает `JustExpired`, очищает blacklist и whitelist. Caller на `JustExpired` пробует primary заново. Если в этот момент EMFILE-источник всё ещё активен — primary connect снова падает с EMFILE, `is_backend_unreachable` возвращает true, blacklist выставляется опять на 30 секунд.

Цикл повторяется столько, сколько длится EMFILE. Каждая итерация даёт:
* лог-сообщение об ошибке connect от каждого client request (rate-limit на этом уровне нет);
* лог-сообщение `fallback discovery failed`;
* инкремент метрик `PATRONI_API_ERRORS_TOTAL`, `FALLBACK_ACTIVE`.

При 3.2K активных клиентах нагрузка на логи составляла оценочно несколько сотен строк в секунду на одну машину.

Главное здесь — в коде нет механизма разорвать этот цикл изнутри. Чтобы EMFILE прошёл, fd должны освободиться. Освобождение возможно если:
* клиенты сами отключатся (приложение их закрыло) — но в их случае приложения retry-ят и держат коннекты;
* код принудительно закроет клиентские коннекты — но такого пути в проекте нет;
* процесс перезапустят — что и пришлось сделать оператору.

Второй вариант — пропуск в архитектуре. При EMFILE на backend connect (или любой другой ресурсной нехватке) надёжный pooler должен жертвовать частью клиентских соединений, чтобы поток мог продолжаться. Сейчас код ничего не делает с клиентами и продолжает писать ошибки в лог тем же темпом, с которым приходят новые запросы.

## Минорное: `recvmsg` без `MSG_CMSG_CLOEXEC`

```rust
// src/client/migration.rs:778
let n = unsafe { libc::recvmsg(socket_fd, &mut msghdr, 0) };
```

Без `MSG_CMSG_CLOEXEC` полученные через SCM_RIGHTS fd в child не имеют `FD_CLOEXEC`. На втором последовательном binary upgrade child→grandchild через exec эти fd попадут в grandchild вместо того чтобы закрыться. Для одиночного upgrade — без последствий, для серии — потенциальный накопительный leak. К этому инциденту прямого отношения не имеет, но фиксируется здесь, потому что трогает тот же миграционный код.

## Что не подтвердилось из ранних гипотез

Зафиксируем гипотезы, которые проверкой кода были отброшены — чтобы не возвращаться:

* **Web UI listener даёт десятки fd**. `bind_web_listener` (`src/web/server/listener.rs:20-41`) — один TCP listener с `set_reuseport(true)`, accept'ит соединения в одну task. Один fd на listener плюс по 1 fd на активное HTTP-соединение (обычно единицы).
* **min_pool_size prewarm в child добавляет сотни backend connections**. У пользователя 10 backend, prewarm даст максимум 10.
* **Child наследует все parent fd через fork+exec**. Tokio listener-сокеты создаются с `SOCK_CLOEXEC`, при exec автоматически закрываются. Parent явно делает `F_SETFD=0` (`src/app/server.rs:1028-1032`) только на `listener_fd`, `pipe_write_fd`, `migration_child_fd` — то есть передаёт ровно те, которые нужны.
* **GC race на dynamic pools (PR #255)**. У пользователя нет dynamic pools.
* **Prometheus scrape держит много fd**. У пользователя нет высокой нагрузки на /metrics.

## План починки

### P0.1: миграционный буфер привязан к nofile-budget

#### Что меняется в поведении

При старте pg_doorman читает текущий `RLIMIT_NOFILE` через `getrlimit`, вычитает ожидаемое количество fd под штатную нагрузку (живые клиенты, backends, listeners, runtime-internal) плюс безопасный запас, и оставшийся budget использует как capacity миграционного канала. Константа 4096 больше не используется как hardcoded ceiling.

Грубо говоря, capacity становится приблизительно `(soft_limit − max_clients − max_backends − 200) / 2`. Делим на 2, потому что каждая запись в очереди — это второй fd в parent (через dup). Cap'им на 4096 сверху (если оператор поставил очень высокий ulimit, не аллоцируем массивный mpsc) и снизу на 64 (чтобы хотя бы какой-то миграционный поток работал).

При профиле пользователя (6200 / 3200 / 10) capacity вычисляется примерно в 1395. Это значит:
* до 1395 клиентов могут мигрировать параллельно через буфер;
* если в очереди уже 1395 — следующий клиент видит `try_send` Err и не мигрирует, остаётся обслуживаться в parent, либо отбрасывается (см. P0.2);
* parent не выходит за свой fd-budget.

#### Анализ безопасности

Параметр capacity влияет только на throughput миграции. Меньшее значение означает что часть клиентов не успевает мигрировать в child за окно `shutdown_timeout` и обрывается при выходе parent. Это **уменьшение количества миграций**, не **поломка работающих**.

Корректность поведения остаётся прежней — payload по-прежнему передаётся через dup, sendmsg, close. Никакой race condition не вводится.

Что обязательно проверить перед PR:
1. На старте код читает `getrlimit(RLIMIT_NOFILE)` до любых возможных модификаций лимита; под runr, systemd и standalone значение должно совпадать с тем, что выставил init.
2. Формула capacity консервативна на пограничных значениях. Особенно — что при очень низком `soft_limit` (например, 1024 в каком-нибудь docker-окружении) формула не даёт capacity < 64 без явного warning'а в лог.
3. Что в config есть способ оператору **снизить** computed capacity ниже автомата (на случай если хочется ручного управления), но **не повысить** выше budget'а (это бы вернуло исходную проблему).

BDD-сценарий, который должен пойти в PR:

```gherkin
@client-migration @fd-budget
Scenario Outline: Migration capacity stays within nofile budget
  Given pg_doorman started with NOFILE soft limit set to <limit>
  And <clients> clients are connected and idle
  When SIGUSR2 triggers binary upgrade
  Then process fd count never exceeds <limit> throughout the handoff
  And at least <expected_migrated> clients are migrated to the child

  Examples:
    | limit | clients | expected_migrated |
    | 6200  | 3200    | 1300              |
    | 8200  | 3200    | 3200              |
    | 16384 | 5000    | 5000              |
```

#### Что НЕ делать

Не оставлять hardcoded `MIGRATION_CHANNEL_CAPACITY=4096`. Не пытаться внутри pg_doorman поднимать `RLIMIT_NOFILE` через `setrlimit` — это маскирует операторскую настройку и не решает архитектурный вопрос.

### P0.2: pg_doorman выходит из затяжного EMFILE

#### Что меняется в поведении

Добавляется watchdog (background-задача), которая:
1. Периодически (например, раз в секунду) сравнивает текущее число открытых fd процесса с soft-лимитом.
2. Если соотношение перешагнуло порог опасной зоны (например, 95 % от soft) и при этом за последние N секунд (например, 5) хотя бы один `connect()` упал с EMFILE — watchdog инициирует graceful shed: принудительно закрывает M процентов самых давних idle-клиентских соединений (или столько, сколько нужно, чтобы вернуть процесс ниже порога).
3. Закрытые клиенты получают понятный FATAL с SQLSTATE, например 53300 — ровно так, как PG отвечает на исчерпание max_connections. Приложения, привыкшие retry-ить на 53300, переподключатся без специальной логики.

После shed-цикла процесс продолжает работать с уменьшенным числом клиентов. Когда нагрузка спадёт (или оператор разберётся с лимитом), всё восстановится.

Семантика: pooler какое-то время логирует ошибки backend connect, потом сам уменьшает число клиентов до уровня, при котором обслуживание возможно, и продолжает работать. Альтернатива текущему поведению «логировать ошибки до ручного restart».

#### Анализ безопасности

Сложность не в самом shed, а в выборе кого жертвовать. Параметры:
* Только idle-клиенты (без активной transaction) — иначе шедить in-transaction клиента означает abort'ить транзакцию посередине, что для приложения может быть хуже чем reconnect.
* Самые давно неактивные первыми — чтобы шедить именно «забытые» соединения, не активную нагрузку.
* Никакого shed для admin-клиентов — оператор должен иметь возможность зайти и разобраться даже под нагрузкой.

Что обязательно проверить:
1. Watchdog не блокирует основной runtime (выделенная tokio task с независимым tick'ом).
2. Метрика порога подсчитывается дёшево — без `ls /proc/self/fd | wc -l` на каждый tick. Можно либо считать самим (атомарный счётчик при accept/close), либо `getdents` с буфером.
3. Shed-операция декларативна — закрываем половинки stream'а через `shutdown` или `drop` task'а, который их держит. Не делаем `libc::close` руками.
4. После shed выдаётся один WARN-лог с числом закрытых клиентов и причиной, не строка на каждого клиента.
5. Нет double-shed: после shed выставляется cooldown (например, 30 секунд), в течение которого следующий shed не запускается, даже если порог снова перешагнут.

BDD-сценарий:

```gherkin
@watchdog @fd-budget
Scenario: pg_doorman drops idle clients when EMFILE persists
  Given pg_doorman started with NOFILE soft limit set to N
  And N − 50 idle clients are connected (very tight budget)
  And a backend connect attempt returns EMFILE
  When 5 seconds pass with EMFILE persisting
  Then pg_doorman closes at least 10 idle clients with SQLSTATE 53300
  And those clients see FATAL "too many connections" (or similar)
  And process fd count drops back below the danger threshold
  And subsequent client connect succeeds within 100ms
```

#### Что НЕ делать

Не убивать клиентов, которые в активной transaction — это разрушает данные приложения хуже чем EMFILE-зависание. Не делать «soft reset» через закрытие admin-listener'а или migration-сокета (это нужно для других путей recovery).

Не привязываться к точному `RLIMIT_NOFILE` cap'у. Watchdog работает по проценту, а не по абсолютному числу, чтобы один и тот же код работал на разных конфигурациях.

### P1: отличать EMFILE от network failures на backend connect

#### Что меняется в поведении

EMFILE при `connect()` к backend больше не классифицируется как «primary unreachable». Patroni-фолбэк не запускается. Клиент получает прямую ошибку с понятным SQLSTATE (53300 или 53400, см. открытые вопросы), и retry в его собственной логике.

Это не убирает EMFILE как таковой (он всё равно возникнет, если budget исчерпан), но убирает ложный сигнал «Patroni сломан» в логах и убирает retry-storm в Patroni discovery.

#### Анализ безопасности

В коде потребуется новый вариант ошибки, например `Error::LocalResourceExhausted(String)`. Несколько мест зависят от текущей категоризации `ConnectError` и потребуют обновления:

1. `FailureReason::from(&Error)` в `src/pool/fallback.rs:85-94` — добавить разбор нового варианта, чтобы fallback-кандидаты с EMFILE отмечались отдельной категорией в cooldown.
2. `process_error` в `src/client/error_handling.rs` — добавить явный case для `LocalResourceExhausted`, иначе клиент получит generic-ошибку (catch-all `_ => Err(err)`). Без этого изменения наблюдаемость на клиенте ухудшится.
3. Опционально метрика `BACKEND_CONNECT_EMFILE_TOTAL{pool}` — после P1 EMFILE-события перестанут отражаться в `FALLBACK_ACTIVE` и без отдельной метрики «пропадут из радаров».

Реальные сценарии fallback, не затрагиваемые:
* Connection refused, network unreachable, timeout — приходят как `io::Error` с другим `raw_os_error`, классифицируются в `ConnectError`, фолбэк работает как раньше.
* PG max_connections — это FATAL 53300 на handshake, попадает в `ServerStartupError`, не в `ConnectError`. Поведение не меняется.

Что обязательно проверить перед PR:
1. `err.raw_os_error()` действительно возвращает `Some(libc::EMFILE)` / `Some(libc::ENFILE)` под tokio `connect()` на актуальном Linux. Можно покрыть unit-тестом, где запускается потомок с искусственно заниженным `RLIMIT_NOFILE`.
2. Все callsite'ы `Error::ConnectError` — `grep -rn 'Error::ConnectError' src/` — проверены, что новая категория обрабатывается, либо явно понятно почему мимо.

BDD-сценарий:

```gherkin
@patroni_fallback @resource-exhaustion
Scenario: EMFILE on primary connect does not trigger Patroni fallback
  Given pg_doorman started with fallback configured and local PG up
  And pg_doorman fd budget is artificially exhausted
  When client requests a backend connection
  Then client receives an error with SQLSTATE 53300 or 53400
  And metric pg_doorman_fallback_active{pool=...} stays 0
  And no entry "fallback discovery failed" appears in the log
```

#### Что НЕ делать

Не угадывать EMFILE по тексту ошибки от reqwest — это хрупкий путь. Все решения по классификации делать на уровне `raw_os_error`.

### Минор: `MSG_CMSG_CLOEXEC` в `recvmsg`

`src/client/migration.rs:778` — заменить `0` на `libc::MSG_CMSG_CLOEXEC`. Однострочный fix. Закрывает потенциальный leak fd при серии последовательных upgrade. К текущему инциденту прямого отношения не имеет, но идёт в том же файле, можно прислать вторым коммитом в P0.1-PR'е.

## Открытые вопросы для обсуждения

1. **SQLSTATE для `LocalResourceExhausted`**. 53300 — самый близкий стандартный код, но семантически он про PG max_connections. Альтернатива — 53400 (configuration_limit_exceeded). Какой из двух — обсудить с командой.

2. **Watchdog: какой процент порога**. 95 % — стартовое предложение. Возможно, нужны два порога: «warn» (например, 80 %, лог-сообщение, никаких действий) и «shed» (например, 95 %, начинаем закрывать клиентов).

3. **Capacity-formula tuning**. Точные коэффициенты в формуле для P0.1 (резерв в 200 fd, делитель 2) — обсудить с командой; в первом PR закладывать конфигурируемое значение или жёсткое.

4. **Метрика `BACKEND_CONNECT_EMFILE_TOTAL`**. Обязательная часть P1 или отдельный PR? Я склоняюсь к обязательной — без неё EMFILE становится «невидимым» после изменения классификатора.

5. **Прогон сценария на стенде**. Прежде чем мерджить любой из P0, повторить сценарий инцидента на стенде с заниженным `LimitNOFILE` и большим числом клиентов — убедиться что после fix'а ситуация не воспроизводится. Без этой проверки гарантии «починили» нет.

## Что НЕ входит в этот документ

* Конкретный код PR'ов — пишем отдельно по каждому пункту после обсуждения safety-анализа.
* Production-mitigation для текущего парка машин (поднять `LimitNOFILE` в их service-юните) — это операционное действие. Внутри pg_doorman делаем так, чтобы он корректно работал в пределах заданного оператором лимита.
* Документация для пользователя про recommended ulimit — обсудим после fix'ов, чтобы дать осмысленный ориентир.
