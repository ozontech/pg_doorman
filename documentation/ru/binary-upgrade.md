# Binary upgrade

Обновление pg_doorman без разрыва клиентских соединений. Старый
процесс передаёт idle-клиентов новому через Unix socket -- клиенты
продолжают работу на том же TCP-соединении без reconnect.

## Быстрый старт

```bash
# 1. Заменить бинарник на диске
cp pg_doorman_new /usr/bin/pg_doorman

# 2. Запустить upgrade
kill -USR2 $(pgrep pg_doorman)

# 3. Проверить: старый PID исчез, клиенты на месте
pgrep pg_doorman   # новый PID
```

Или через admin-консоль:

```sql
UPGRADE;
```

## Как работает upgrade

```
                        SIGUSR2
                           |
                           v
               +-----------------------+
               | 1. Валидация конфига  |
               |    (pg_doorman -t)    |   -- fail --> отмена, продолжаем
               +-----------+-----------+
                           |
                           v
               +-----------------------+
               | 2. Запуск нового      |
               |    socketpair()       |
               |    inherit-fd         |
               |    readiness pipe     |   -- ожидание до 10с
               +-----------+-----------+
                           |
             +-------------+-------------+
             |                           |
             v                           v
  +---------------------+    +---------------------+
  | СТАРЫЙ процесс      |    | НОВЫЙ процесс       |
  |                     |    |                     |
  | 3. Idle-клиенты     |    | migration_receiver  |
  |    сериализация     +--->+    восстановление   |
  |    dup() + SCM_RIGHTS    |    запуск client    |
  |                     |    |    handle()         |
  | 4. Клиенты в tx     |    |                     |
  |    дождаться COMMIT +--->+ Принимает новые     |
  |    мигрировать      |    | соединения          |
  |                     |    |                     |
  | 5. Shutdown timer   |    +---------------------+
  |    опрос 250мс      |
  |    выход при 0      |
  +---------------------+
```

### Фаза 1: Валидация конфига

Текущий бинарник перезапускается с флагом `-t` и конфигом.
Если валидация проваливается -- upgrade отменяется, старый процесс
продолжает обслуживать трафик. В логах баннер:

```
!!!  BINARY UPGRADE ABORTED - SHUTDOWN CANCELLED  !!!
!!!  FIX THE CONFIGURATION BEFORE ATTEMPTING BINARY UPGRADE AGAIN  !!!
!!!  THE SERVER WILL CONTINUE RUNNING WITH THE CURRENT BINARY  !!!
```

### Фаза 2: Запуск нового процесса

**Foreground mode:**

1. Создаётся Unix `socketpair()` для миграции клиентов.
2. Listener fd передаётся дочернему процессу через `--inherit-fd`.
3. Readiness pipe: родитель ждёт до 10 секунд байт от дочернего
   процесса. Дочерний пишет в pipe, когда начинает принимать
   соединения.
4. Родитель закрывает свой listener -- новые соединения идут
   в дочерний процесс.

**Daemon mode:**

Запускается новый daemon-процесс. Старый закрывает listener.
Миграция клиентов через socketpair не используется -- клиенты
дренируются (получают error 58006 при истечении `shutdown_timeout`).

### Фаза 3: Миграция idle-клиентов (foreground)

Когда установлен флаг `MIGRATION_IN_PROGRESS`, каждый idle-клиент
(нет активной транзакции, нет pending deferred `BEGIN`, нет
буферизованных данных на чтение) мигрирует:

1. **Сериализация**: connection_id, secret_key, имя пула, username,
   server parameters, полный кэш prepared statements.
2. **dup() + SCM_RIGHTS**: TCP socket fd дублируется и передаётся
   новому процессу через Unix socketpair.
3. **Восстановление**: новый процесс пересоздаёт Client struct,
   подключает к нужному пулу и запускает `handle()`.

Клиент не замечает миграции. Никакого reconnect, никакого error,
никакой повторной аутентификации. TCP-соединение -- тот же
физический socket.

### Фаза 4: Дренирование in-transaction клиентов

Клиент внутри `BEGIN ... COMMIT` продолжает работать на старом
процессе. Его серверное соединение остаётся живым. После завершения
транзакции (COMMIT или ROLLBACK) клиент становится idle и мигрирует
на следующей итерации цикла.

Deferred `BEGIN` (сервер ещё не выделен) тоже блокирует миграцию.
Клиент должен отправить запрос (сбросив deferred BEGIN), затем
COMMIT, и только потом мигрирует.

### Фаза 5: Shutdown timer

Shutdown timer опрашивает `CURRENT_CLIENT_COUNT` каждые 250 мс.
Когда все клиенты мигрировали или отключились -- старый процесс
вызывает `process::exit(0)`.

Если `shutdown_timeout` истекает раньше -- принудительный выход,
оставшиеся соединения закрываются.

Во время миграции `drain_all_pools()` откладывается: in-transaction
клиентам нужны их серверные соединения. Дренирование пулов начинается
только после завершения миграции или сброса `MIGRATION_IN_PROGRESS`.

## Prepared statements

Кэш prepared statements каждого клиента сериализуется при миграции:

- Ключ statement (именованный или anonymous hash)
- Hash запроса
- Полный текст запроса
- OID типов параметров

В новом процессе:

1. Каждая запись регистрируется в pool-level shared cache (DashMap).
2. Серверные бэкенды свежие -- на них нет prepared statements.
3. При первом `Bind` к мигрированному statement pg_doorman прозрачно
   отправляет `Parse` на новый бэкенд. Клиент не видит дополнительного
   round-trip.

**Ограничения:**

- Если `client_prepared_statements_cache_size` нового конфига меньше,
  чем количество entries у клиента -- лишние вытесняются (LRU).
  Оставшиеся работают нормально.
- Anonymous prepared statements (`Parse` с пустым именем) переживают
  миграцию, но требуют повторного `Parse` перед `Bind` в новом процессе.
- `DEALLOCATE ALL` после миграции очищает переданный кэш. Повторный
  `Parse` с тем же именем использует новый текст запроса.

## TLS migration

По умолчанию TLS-клиенты не мигрируют -- зашифрованная сессия
требует ключевой материал, который живёт внутри OpenSSL state machine.
Такие клиенты дренируются при upgrade: соединение закрывается при
истечении `shutdown_timeout`, клиент переподключается к новому процессу.

Opt-in фича `tls-migration` решает эту проблему. Патченный OpenSSL
экспортирует symmetric cipher state, передаёт его вместе с fd через
Unix socket, а новый процесс импортирует состояние и продолжает
шифрование. Клиент не делает повторный TLS handshake.

### Что экспортируется

Патч добавляет `SSL_export_migration_state()` и
`SSL_import_migration_state()` в OpenSSL 3.5.5. Экспортируемые данные:

- Версия TLS-протокола
- ID cipher suite и tag length
- Symmetric keys для чтения/записи (входные данные для AES key
  schedule, не развёрнутые)
- IV (nonce) для чтения/записи
- Sequence numbers для чтения/записи (по 8 байт)
- Для TLS 1.3: server и client application traffic secrets

Этого достаточно для восстановления record layer в новом процессе
и продолжения шифрования/дешифрования на том же TCP-соединении.

### Сборка с TLS migration

```bash
cargo build --release --features tls-migration
```

Требует `perl` и `patch` в build-окружении. Vendored OpenSSL 3.5.5
собирается из исходников с наложенным патчем.

### Offline-сборка (без доступа к интернету)

```bash
# Скачать tarball заранее
curl -fLO https://github.com/openssl/openssl/releases/download/openssl-3.5.5/openssl-3.5.5.tar.gz

# Собрать с указанием пути
OPENSSL_SOURCE_TARBALL=./openssl-3.5.5.tar.gz \
  cargo build --release --features tls-migration
```

SHA-256 tarball'а проверяется автоматически.

### Ограничения

- **Linux only.** На macOS/Windows TLS migration не поддерживается
  (native-tls использует Security.framework / SChannel, не OpenSSL).
- **Одинаковые сертификаты.** Старый и новый процесс должны
  использовать одни и те же `tls_private_key` и `tls_certificate`.
  Cipher state привязан к SSL_CTX, созданному из сертификата.
  Изменённые сертификаты приводят к ошибке импорта и отключению
  клиента.
- **FIPS несовместимо.** Vendored OpenSSL не проходит FIPS-валидацию.
  Для FIPS используйте сборку без `tls-migration` (TLS-клиенты
  будут дренироваться вместо миграции).
- **Нет HSM/PKCS#11.** Vendored OpenSSL собирается с `no-engine`.

### Известные ограничения

- **KeyUpdate (TLS 1.3) не поддерживается.** Если клиент или
  pg_doorman отправит KeyUpdate, экспортированные ключи станут
  неактуальными. На практике libpq и PostgreSQL не отправляют
  KeyUpdate. Кастомные клиенты с агрессивной ротацией ключей
  могут быть затронуты.
- **SSL_pending данные не проверяются.** Миграция происходит в idle
  point, где нет буферизованных данных приложения. Инвариант idle
  point это гарантирует, но явная проверка `SSL_pending()` не
  выполняется.
- **Привязка к OpenSSL 3.5.5.** Патч модифицирует внутренние
  структуры OpenSSL (`ssl_local.h`, `rec_layer_s3.c`, `ssl_lib.c`).
  При обновлении OpenSSL нужно проверить и переложить патч на новую
  версию.

## Сигналы

| Сигнал | Поведение |
|--------|-----------|
| `SIGUSR2` | Binary upgrade + graceful shutdown. **Рекомендуемый для всех режимов.** |
| `SIGINT` | В foreground + TTY (Ctrl+C): только shutdown, без upgrade. В daemon / non-TTY: binary upgrade (legacy-совместимость). |
| `SIGTERM` | Немедленный выход. Транзакции обрываются. Все клиенты отключаются. |
| `SIGHUP` | Перечитать конфигурацию без перезапуска. Без простоя. |
| `UPGRADE` (admin) | Отправляет SIGUSR2 текущему процессу. Тот же эффект. |

> **Legacy-поведение SIGINT:**
> SIGINT запускает binary upgrade в daemon mode или без TTY (например, под systemd). В интерактивном терминале Ctrl+C останавливает процесс без запуска нового. Используйте `kill -USR2` или `UPGRADE` в admin-консоли для binary upgrade в foreground mode.

## Daemon vs foreground

| | Foreground | Daemon |
|---|---|---|
| Миграция клиентов через fd passing | Да (socketpair) | Нет |
| Idle-клиенты сохраняются | Да | Нет (drain с 58006) |
| In-tx клиенты | Завершают tx, затем мигрируют | Завершают tx, затем 58006 |
| Запуск нового процесса | Наследует listener fd | Запускается независимо |
| Рекомендуется для | systemd, контейнеры, k8s | Legacy-установки |

Для zero-downtime upgrade с миграцией клиентов запускайте в foreground
mode. systemd управляет жизненным циклом процесса:

```ini
[Service]
Type=simple
ExecStart=/usr/bin/pg_doorman /etc/pg_doorman.yaml
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

## Конфигурация

### `shutdown_timeout`

Максимальное время ожидания завершения транзакций перед принудительным
закрытием соединений. Старый процесс завершается по истечении этого
таймаута вне зависимости от оставшихся клиентов.

По умолчанию: 10 секунд.

Рекомендация для production с длинными аналитическими запросами:
30-60 секунд.

```toml
[general]
shutdown_timeout = 60000  # миллисекунды
```

Слишком маленькое значение -- риск убить активные транзакции. Слишком
большое -- задержка выхода старого процесса при зависшем клиенте
(например, idle-in-transaction). Выбирайте значение, покрывающее
самую длинную ожидаемую транзакцию, с запасом.

### `tls_private_key` / `tls_certificate`

Для TLS migration оба процесса (старый и новый) загружают одни и
те же файлы. Если сертификат поменялся между версиями бинарника --
TLS-клиенты получат ошибку при импорте cipher state и будут
отключены.

Ротацию сертификатов делайте через `SIGHUP` (reload конфига) до
binary upgrade.

### `prepared_statements_cache_size`

Pool-level кэш prepared statements. Напрямую на миграцию не
влияет, но pool cache в новом процессе должен быть достаточного
размера для entries от мигрированных клиентов.

### `client_prepared_statements_cache_size`

Per-client кэш prepared statements. Клиентский кэш сериализуется
полностью при миграции. Если новый конфиг имеет меньшее значение --
LRU вытесняет лишние записи.

## Мониторинг

### Логи

Ключевые строки в логах при миграции:

```
INFO  Got SIGUSR2, starting binary upgrade and graceful shutdown
INFO  Validating configuration with: /usr/bin/pg_doorman -t pg_doorman.yaml
INFO  Configuration validation successful
INFO  Starting new process with inherited listener fd=5
INFO  New process signaled readiness
INFO  Client migration enabled
INFO  [user@pool #c42] client 10.0.0.1:51234 migrated to new process
INFO  waiting for 3 clients in transactions
INFO  All clients disconnected, shutting down
INFO  Migration sender finished
```

В новом процессе:

```
INFO  migration receiver: listening for migrated clients
INFO  [user@pool #c42] migrated client accepted from 10.0.0.1:51234
INFO  migration receiver done: migration socket closed
INFO  migration receiver: stopped
```

### Prometheus-метрики

| Метрика | Значение при upgrade |
|---------|----------------------|
| `pg_doorman_pools_clients{status="active"}` | Должна упасть до 0 на старом процессе |
| `pg_doorman_pools_clients{status="idle"}` | Падает по мере миграции клиентов |
| `pg_doorman_connection_count{type="total"}` | Старый: убывает, новый: растёт |
| `pg_doorman_clients_prepared_cache_entries` | Подтверждает перенос кэша |

### Admin-консоль

```sql
-- На новом процессе (старый отклоняет не-admin соединения)
SHOW POOLS;
SHOW CLIENTS;
```

## Troubleshooting

### Клиент получил "pooler is shut down now" (58006) вместо миграции

**Ctrl+C в foreground mode.** SIGINT в TTY = shutdown без upgrade.
Используйте `kill -USR2` или `UPGRADE` в admin-консоли.

**Daemon mode.** Daemon mode не использует fd-based миграцию.
Клиенты дренируются. Переключитесь на foreground mode.

**`PG_DOORMAN_CI_SHUTDOWN_ONLY=1` установлен.** Эта переменная
окружения принудительно включает shutdown-only mode (используется
в CI-тестах). Уберите её.

### Старый процесс не завершается

**Длинная транзакция.** Клиент застрял в `BEGIN` без `COMMIT`.
Дождитесь `shutdown_timeout` или завершите транзакцию вручную.

**Admin-соединения.** Admin-соединения не мигрируются. Закройте
admin-сессию на старом процессе.

**Принудительный выход:** `kill -TERM <old_pid>` отправляет SIGTERM.

### TLS-соединение оборвалось после upgrade

**Бинарник собран без `--features tls-migration`.** TLS-клиенты
дренируются вместо миграции. Пересоберите с feature flag.

**Запуск не на Linux.** TLS migration работает только на Linux.

**Сертификат/ключ изменились.** Старый процесс экспортировал cipher
state, привязанный к старому сертификату. Используйте те же файлы
для обоих процессов. Ротацию сертификатов делайте через SIGHUP
до binary upgrade.

### "TLS migration not available" в логах

Новый процесс получил миграционный payload с TLS-данными, но собран
без `--features tls-migration` или запущен не на Linux. Клиент
отключается. Пересоберите новый бинарник с feature flag.

### "migration channel not ready" в логах

Канал `MIGRATION_TX` ещё не инициализирован. Новый процесс не
завершил запуск, когда клиент попытался мигрировать. Клиент
повторит попытку на следующей idle-итерации (через миллисекунды).

### "migration channel send failed" в логах

Канал миграции переполнен (capacity: 4096). Возможно при одновременной
миграции тысяч клиентов. Клиент повторит попытку на следующей
idle-итерации.

### "prepare_migration failed" в логах

Raw fd клиента недоступен или `dup()` не удался. Возможные причины:
исчерпание файловых дескрипторов, или клиент подключился через
code path, который не сохраняет raw fd. Проверьте `ulimit -n`.

> **Совместимость с клиентскими библиотеками:**
> Библиотеки вроде `github.com/lib/pq` или Go `database/sql` могут
> потребовать настройки для обработки reconnect при получении error
> 58006 (для клиентов в daemon mode или застрявших дольше
> `shutdown_timeout`). См. [issue](https://github.com/lib/pq/issues/939).

## Чек-лист перед production

Перед выкатом binary upgrade в production:

- [ ] Запуск в **foreground mode** (не daemon) для fd-based миграции
- [ ] `shutdown_timeout` покрывает самую длинную ожидаемую транзакцию
      (рекомендация: 30-60 секунд для OLTP, больше для аналитики)
- [ ] Если используете TLS: сборка с `--features tls-migration`,
      оба процесса используют одинаковые файлы сертификата и ключа
- [ ] Протестировать upgrade в staging: открыть сессию, отправить
      SIGUSR2, убедиться что сессия продолжает работать
- [ ] В systemd unit есть `ExecReload=/bin/kill -SIGUSR2 $MAINPID`
- [ ] Мониторинг логов на ошибки миграции после первого production
      upgrade
- [ ] Подтвердить что старый процесс завершился (PID file или `pgrep`)
- [ ] Проверить Prometheus-метрики: клиенты на новом процессе
