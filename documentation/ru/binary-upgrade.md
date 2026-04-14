# Binary upgrade

Обновление pg_doorman без разрыва клиентских соединений. Старый
процесс передаёт idle-клиентов новому через Unix socket, клиенты
продолжают работу без reconnect.

Аудитория: DBA или инженер эксплуатации, который настраивает
zero-downtime деплой pg_doorman в production.

## Сигналы

| Сигнал | Поведение |
|--------|-----------|
| `SIGUSR2` | Binary upgrade + graceful shutdown. **Рекомендуемый.** |
| `SIGINT` | В foreground + TTY (Ctrl+C): только shutdown, без upgrade. В daemon / non-TTY: binary upgrade (legacy-совместимость). |
| `SIGTERM` | Немедленный выход. Транзакции обрываются. |
| `SIGHUP` | Перечитать конфигурацию без перезапуска. |

```bash
# Запустить binary upgrade
kill -USR2 $(pgrep pg_doorman)

# Или через admin-консоль
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c "UPGRADE;"
```

### systemd

```ini
[Service]
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

`systemctl reload pg_doorman` запускает binary upgrade.

## Что происходит при upgrade

### 1. Валидация конфига

Текущий бинарник перезапускается с флагом `-t` и новым конфигом.
Если валидация проваливается — upgrade отменяется, старый процесс
продолжает обслуживать трафик. В логах появляется баннер с ошибкой.

### 2. Запуск нового процесса

**Foreground mode:** создаётся Unix socketpair для миграции клиентов.
Listener socket передаётся дочернему процессу через `--inherit-fd`.
Родитель ждёт до 10 секунд сигнала готовности от дочернего процесса,
затем закрывает свой listener.

**Daemon mode:** запускается новый daemon-процесс. Старый daemon
закрывает listener. Миграция клиентов через socketpair не используется —
старые клиенты дренируются.

### 3. Миграция idle-клиентов (foreground)

Клиенты, которые не находятся внутри транзакции, мигрируют в новый
процесс:

1. pg_doorman сериализует состояние клиента: `connection_id`,
   `secret_key`, имя пула, username, server parameters, кэш
   prepared statements.
2. TCP socket дублируется через `dup()` и передаётся новому процессу
   через `SCM_RIGHTS`.
3. Новый процесс восстанавливает клиента и подключает его к свежему
   backend-пулу.

Клиент не замечает миграции — TCP-соединение остаётся тем же.
Никакого reconnect, никакого error 58006.

### 4. Клиенты в транзакциях

Клиент внутри `BEGIN ... COMMIT` продолжает работать на старом
процессе. После завершения транзакции (COMMIT или ROLLBACK) клиент
мигрирует в новый процесс на следующей idle-итерации.

Если `shutdown_timeout` истекает до завершения транзакции — старый
процесс принудительно закрывает соединение.

### 5. Завершение старого процесса

Shutdown timer опрашивает счётчик клиентов каждые 250 мс. Когда
все клиенты мигрировали или отключились — старый процесс завершается.
Если прошло `shutdown_timeout` секунд — принудительный выход.

## Prepared statements

Кэш prepared statements каждого клиента сериализуется при миграции:
имена, query hash, полный текст запроса, типы параметров.

В новом процессе:
- Записи регистрируются в pool-level кэше (DashMap) нового процесса.
- Серверные бэкенды нового процесса свежие — на них нет prepared
  statements.
- При первом `Bind` к мигрированному statement pg_doorman прозрачно
  отправляет `Parse` на новый бэкенд. Клиент этого не видит.

Ограничения:
- Если `prepared_statements_cache_size` нового конфига меньше, чем
  количество statements у клиента — лишние теряются (LRU eviction).
- Anonymous prepared statements (`Parse` с пустым именем) переживают
  миграцию, но требуют повторного `Parse` перед `Bind` в новом
  процессе.

## TLS migration

По умолчанию TLS-клиенты дренируются при upgrade — их TCP socket
передаётся, но зашифрованная сессия не может продолжиться без
ключевого материала.

Opt-in фича `tls-migration` решает это: патченный OpenSSL
экспортирует symmetric cipher state (ключи, IV, sequence numbers),
передаёт его вместе с socket, а новый процесс импортирует состояние
и продолжает шифрование. Клиент не делает повторный TLS handshake.

### Сборка

```bash
cargo build --release --features tls-migration
```

Требует Perl и `patch` в build-окружении — vendored OpenSSL 3.5.5
собирается из исходников.

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
  Cipher state привязан к SSL_CTX, который создаётся из сертификата.
- **FIPS несовместимо.** Vendored OpenSSL не проходит FIPS-валидацию.
  Для FIPS используйте сборку без `tls-migration` (системный OpenSSL).
- **Нет HSM/PKCS#11.** Vendored OpenSSL собирается с `no-engine`.

## Конфигурация

### `shutdown_timeout`

Максимальное время ожидания завершения транзакций перед принудительным
выходом старого процесса. По умолчанию: 10 секунд.

Рекомендация: 30-60 секунд для production, если есть длинные
аналитические запросы.

```toml
[general]
shutdown_timeout = 60000  # 60 секунд, в миллисекундах
```

### `tls_private_key` / `tls_certificate`

Для TLS migration оба процесса загружают одни и те же файлы. Если
сертификат поменялся между версиями — TLS-клиенты получат ошибку
при импорте cipher state.

### `prepared_statements_cache_size`

Клиентский кэш prepared statements сериализуется полностью. Если
новый конфиг имеет меньший `client_prepared_statements_cache_size`,
LRU отбросит часть записей.

## Troubleshooting

### Клиент получил "pooler is shut down now" (58006) вместо миграции

Причины:
- **Ctrl+C в foreground mode.** SIGINT в TTY = чистый shutdown без
  binary upgrade. Используйте `kill -USR2`.
- **Daemon mode без migration socketpair.** Daemon mode не использует
  fd-based миграцию — клиенты дренируются.
- **`PG_DOORMAN_SHUTDOWN_ONLY=1` установлен.** Принудительный
  shutdown-only mode.

### Старый процесс не завершается

- Клиент застрял в длинной транзакции. Дождитесь `shutdown_timeout`
  или завершите транзакцию вручную.
- Admin-соединения не мигрируются. Закройте admin-сессию.
- Отправьте `SIGTERM` для немедленного выхода:
  `kill -TERM <pid>`

### TLS-соединение оборвалось после upgrade

- Бинарник собран без `--features tls-migration`. Пересоберите.
- Запуск не на Linux. TLS migration работает только на Linux.
- Сертификат/ключ изменились между версиями. Используйте те же файлы.

### "TLS migration not available" в логах

Новый процесс получил миграционный payload с TLS-данными, но собран
без `--features tls-migration` или запущен не на Linux. Клиент
отключается. Пересоберите с feature flag.

### "migration channel send failed" в логах

Канал миграции переполнен (capacity: 4096). Возможно при одновременной
миграции тысяч клиентов. Клиент повторит попытку на следующей
idle-итерации.
