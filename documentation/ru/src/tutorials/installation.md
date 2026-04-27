# Установка PgDoorman

PgDoorman работает на Linux и macOS. Для production рекомендуем собирать самим — так вы контролируете версию Rust, целевую платформу и зависимости. Также доступны готовые пакеты из репозиториев и статические бинарники. Docker — только для тестов.

## Системные требования

- Linux (рекомендуется) или macOS
- PostgreSQL 10 или новее (любая поддерживаемая версия)
- Память пропорциональна размеру пулов (несколько МБ на пул + кэш prepared statements)
- Rust 1.87 или новее, если собираете из исходников

## Сборка из исходников (рекомендуется)

Соберите со своим toolchain — это даёт контроль над версией компилятора, целевой платформой и зависимостями:

```bash
git clone https://github.com/ozontech/pg_doorman.git
cd pg_doorman
cargo build --release
sudo install -m 0755 target/release/pg_doorman /usr/local/bin/pg_doorman
```

`cargo build --release` собирает оптимизированный бинарник в `target/release/pg_doorman`. Требования к окружению и процесс разработки описаны в [Участие в проекте](./contributing.md).

### Cargo features

| Feature | По умолчанию | Эффект |
| --- | --- | --- |
| `tls-migration` | выкл | Vendored OpenSSL 3.5.5 с патчем, позволяющим TLS-клиентам пережить обновление бинарника. **Нужен для zero-downtime перезапуска TLS-клиентов.** |
| `pam` | выкл | Поддержка аутентификации PAM (Linux). |

### Сборка с миграцией TLS-клиентов

По умолчанию TLS-клиенты не могут перейти на новый процесс при обновлении бинарника — они получают ошибку `58006` и переподключаются. Чтобы соединения переходили на новый процесс без разрыва, соберите с фичей `tls-migration`:

```bash
cargo build --release --features tls-migration
```

Сборка использует vendored OpenSSL 3.5.5 с патчем, который экспортирует и заново импортирует состояние TLS-шифров (ключи, IV, sequence numbers, TLS 1.3 traffic secrets) при передаче соединений между процессами. Зашифрованные клиенты остаются на том же TCP-соединении без повторного TLS handshake.

**Требования:**

- Только Linux (macOS и Windows используют системный TLS, не OpenSSL).
- Утилиты `perl` и `patch` в `PATH`.
- Около 5 минут дополнительного времени сборки на компиляцию OpenSSL.

**Офлайн-сборка (air-gapped среды):**

```bash
curl -fLO https://github.com/openssl/openssl/releases/download/openssl-3.5.5/openssl-3.5.5.tar.gz
OPENSSL_SOURCE_TARBALL=$(pwd)/openssl-3.5.5.tar.gz \
  cargo build --release --features tls-migration
```

Старый и новый процесс должны использовать одни и те же `tls_certificate` и `tls_private_key`. Полное описание upgrade-процесса, мониторинг и диагностика — в [Graceful Binary Upgrade → TLS migration](./binary-upgrade.md#tls-migration).

Для упаковки в deb/rpm смотрите каталоги `debian/` и `pkg/` в репозитории. Пример `Dockerfile.ubuntu22-tls` собирает образ с поддержкой TLS migration на Ubuntu 22.04.

## Пакеты из репозиториев

Готовые deb- и rpm-пакеты публикуются с теми же релизными тегами. Используйте их, когда сборка из исходников нежелательна.

```admonish warning title="В пакетах нет поддержки TLS"
Пакеты из Ubuntu PPA и Fedora COPR собираются **без поддержки TLS**. Если нужен TLS — для клиентских соединений, серверных соединений к PostgreSQL или для горячей миграции TLS при обновлении бинарника — собирайте из исходников с включённой TLS-фичей. См. [Сборка из исходников](#сборка-из-исходников-рекомендуется) выше.
```

### Ubuntu / Debian (PPA)

```bash
sudo add-apt-repository ppa:vadv/pg-doorman
sudo apt update
sudo apt install pg-doorman
```

Поддерживаемые релизы: `jammy` (22.04 LTS), `noble` (24.04 LTS), `questing` (25.10), `resolute` (26.04 LTS).

### Fedora / RHEL / CentOS / Rocky / AlmaLinux (COPR)

```bash
sudo dnf copr enable @pg-doorman/pg-doorman
sudo dnf install pg_doorman
```

Поддерживаемые цели: Fedora 39, 40, 41; EPEL 8 и 9 для семейства RHEL.

Пакет ставит systemd-юнит, конфиг по умолчанию и пользователя `pg_doorman`.

## Готовые бинарники с GitHub Releases

Если ни сборка из исходников, ни пакеты из репозиториев не подходят, скачайте статический бинарник со [страницы релизов](https://github.com/ozontech/pg_doorman/releases):

```bash
# Замените VERSION и TARGET на нужные значения со страницы релизов.
curl -L -o pg_doorman \
  "https://github.com/ozontech/pg_doorman/releases/download/VERSION/pg_doorman-TARGET"
curl -L -o pg_doorman.sha256 \
  "https://github.com/ozontech/pg_doorman/releases/download/VERSION/pg_doorman-TARGET.sha256"
sha256sum -c pg_doorman.sha256                    # должно вывести "OK"
chmod +x pg_doorman
sudo mv pg_doorman /usr/local/bin/
```

Пропуск checksum-шага означает доверие сетевому пути между вами и `objects.githubusercontent.com`. Не делайте так.

## Docker (только для тестов)

Docker поддерживается для разработки, CI и быстрых демо. Для production не рекомендуется — упаковка и управление жизненным циклом проще через пакеты из репозиториев выше.

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.yaml:/etc/pg_doorman/pg_doorman.yaml \
  ghcr.io/ozontech/pg_doorman
```

`docker-compose.yaml` с PostgreSQL в качестве sidecar лежит в [`example/`](https://github.com/ozontech/pg_doorman/tree/master/example) — для smoke-тестов.

## Проверка установки

```bash
pg_doorman --version
pg_doorman -t /etc/pg_doorman/pg_doorman.yaml   # проверяет конфиг
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c "SHOW VERSION;"
```

`pg_doorman -t` проверяет конфиг до деплоя — у PgBouncer и Odyssey такой возможности нет.

## Куда дальше

- [Базовое использование](./basic-usage.md) — первый конфиг, admin-консоль, мониторинг.
- [Аутентификация](../authentication/overview.md) — выбор подходящего метода.
- [Сигналы и перезагрузка](../operations/signals.md) — сигналы, reload, интеграция с systemd.
- [Graceful Binary Upgrade](./binary-upgrade.md) — замена бинарника без потери клиентов.
