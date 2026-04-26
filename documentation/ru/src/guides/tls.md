# TLS

pg_doorman терминирует TLS на клиентской стороне (клиенты → pg_doorman) и инициирует TLS на серверной стороне (pg_doorman → PostgreSQL). Стороны настраиваются независимо.

## Клиентский TLS

Шифрование соединений между клиентскими приложениями и pg_doorman.

### Режимы

| Режим | Поведение |
| --- | --- |
| `disable` | Не анонсировать TLS. Клиенты, отправляющие `SSLRequest`, получают `'N'` (отказ). |
| `allow` | Анонсировать TLS, но принимать и обычный TCP. |
| `require` | Требовать TLS. Обычные соединения разрываются после неудачного `SSLRequest`. |
| `verify-full` | Требовать TLS и валидный клиентский сертификат. Используется для mTLS. |

`verify-full` — это mTLS: сервер проверяет сертификат клиента. Подготовьте набор клиентских CA через `tls_ca_cert`.

### Конфигурация

```yaml
general:
  tls_mode: "require"
  tls_certificate: "/etc/pg_doorman/tls/server.crt"
  tls_private_key: "/etc/pg_doorman/tls/server.key"
  tls_ca_cert: "/etc/pg_doorman/tls/client_ca.pem"   # только для verify-full
  tls_rate_limit_per_second: 100                       # необязательное ограничение скорости handshake
```

Для разработки сертификат может быть самоподписанным; в продакшене обычно используют Let's Encrypt или внутренний CA.

### Перезагрузка (клиентская сторона)

Клиентские сертификаты загружаются при старте. Их смена требует рестарта процесса. Для клиентского TLS перезагрузки по `SIGHUP` нет.

Для ротации сертификатов без простоя смотрите [Binary Upgrade](../tutorials/binary-upgrade.md).

### Политика шифров

Минимальный TLS 1.2. Список шифров — Mozilla «intermediate»; настройке не подлежит. Direct TLS handshake (PG17, без `SSLRequest`) не поддерживается.

Для управления шифрами TLS 1.3 или direct TLS из PG17 используйте PgBouncer 1.25+.

## Серверный TLS

Шифрование соединений между pg_doorman и бэкендами PostgreSQL. Появилось в 3.6.0.

### Режимы

| Режим | Поведение |
| --- | --- |
| `disable` | Обычный TCP. |
| `allow` (по умолчанию) | Сначала пробовать обычный TCP; если сервер отказывает, повторить попытку на новом сокете с TLS. Соответствует `libpq sslmode=allow`. |
| `prefer` | Отправить `SSLRequest`; если сервер отвечает `'N'`, откатиться к обычному TCP. |
| `require` | Требовать TLS. Падать, если сервер его не поддерживает. |
| `verify-ca` | Требовать TLS и проверять серверный сертификат против настроенного CA. |
| `verify-full` | Требовать TLS, проверять CA и проверять hostname сервера против сертификата. |

`allow` — режим по умолчанию ради обратной совместимости: существующие развёртывания, где у PostgreSQL настроен TLS, автоматически переходят на TLS без изменения конфигурации. Для новых развёртываний, которым нужны явные гарантии, выбирайте `require` или `verify-full`.

### Конфигурация

```yaml
general:
  server_tls_mode: "verify-full"
  server_tls_ca_cert: "/etc/pg_doorman/tls/pg_ca_bundle.pem"

# Необязательно: клиентский сертификат для mTLS к PostgreSQL
  server_tls_certificate: "/etc/pg_doorman/tls/pg_client.crt"
  server_tls_private_key: "/etc/pg_doorman/tls/pg_client.key"
```

`server_tls_ca_cert` принимает PEM-bundle (несколько CA-сертификатов, склеенных подряд). Загружаются все.

### Горячая перезагрузка

По `SIGHUP` серверные сертификаты перечитываются с диска. Существующие соединения продолжают пользоваться исходным TLS-контекстом; новые соединения используют перезагруженные сертификаты. Перезагрузка lock-free через `Arc<ArcSwap<...>>` — без обрыва соединений, без задержек на handshake.

```bash
kill -HUP $(pidof pg_doorman)
```

Это единственный путь перезагрузки TLS. Клиентские сертификаты по `SIGHUP` не перезагружаются.

### mTLS к PostgreSQL

Задайте `server_tls_certificate` и `server_tls_private_key`. PostgreSQL должен быть настроен с `ssl_ca_file`, соответствующим подписанту клиентского сертификата, а у роли в `pg_hba.conf` со стороны PostgreSQL должно быть `clientcert=verify-ca` (или `verify-full`).

## Observability

Серверный TLS покрывают три серии Prometheus:

| Метрика | Тип | Назначение |
| --- | --- | --- |
| `pg_doorman_server_tls_connections` | gauge на пул | Число активных TLS-соединений к PostgreSQL. |
| `pg_doorman_server_tls_handshake_duration_seconds` | histogram на пул | Бакеты продолжительности handshake. |
| `pg_doorman_server_tls_handshake_errors_total` | counter на пул | Неудавшиеся handshake. Алерт при ненулевой скорости. |

Смотрите [Справочник Prometheus](../reference/prometheus.md).

## Известные ограничения

- Протокол `COPY` поверх серверного TLS не покрыт BDD-тестами. Поведение должно работать, но не верифицировано.
- Cancel-запросы к бэкенду минуют серверный TLS — они идут по свежему обычному TCP-соединению. Это совпадает с дизайном протокола PostgreSQL (cancel отправляется по отдельному сокету).
- Direct TLS handshake (быстрый handshake PG17 без `SSLRequest`) не поддерживается ни на одной из сторон.

## Куда дальше

- Настройка нового кластера? Смотрите [Установку](../tutorials/installation.md).
- Ротация сертификатов? Смотрите [Binary Upgrade](../tutorials/binary-upgrade.md) и [Сигналы](../operations/signals.md).
- Hardening существующего развёртывания? Сочетайте с [pg_hba.conf](../authentication/hba.md): принудительный `hostssl` для нелокальных соединений.
