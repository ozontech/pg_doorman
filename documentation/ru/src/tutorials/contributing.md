# Вклад в PgDoorman

Спасибо за интерес к развитию PgDoorman! Это руководство поможет настроить окружение для разработки и разобраться в процессе вклада в проект.

## С чего начать

### Зависимости

Чтобы запускать интеграционные тесты, нужно только:

- [Docker](https://docs.docker.com/get-docker/) (обязательно)
- [Make](https://www.gnu.org/software/make/) (обязательно)

**Установка Nix НЕ требуется** -- воспроизводимость тестового окружения обеспечивается Docker-контейнерами, собранными через Nix.

Для локальной разработки (опционально):
- [Rust](https://www.rust-lang.org/tools/install) (последняя стабильная версия)
- [Git](https://git-scm.com/downloads)

### Настройка окружения для разработки

1. **Сделайте fork репозитория** на GitHub.
2. **Склонируйте свой fork**:
   ```bash
   git clone https://github.com/YOUR-USERNAME/pg_doorman.git
   cd pg_doorman
   ```
3. **Добавьте upstream-репозиторий**:
   ```bash
   git remote add upstream https://github.com/ozontech/pg_doorman.git
   ```

## Локальная разработка

1. **Сборка проекта**:
   ```bash
   cargo build
   ```

2. **Сборка для performance-тестов**:
   ```bash
   cargo build --release
   ```

3. **Настройка PgDoorman**:
   - Скопируйте пример конфигурации: `cp pg_doorman.toml.example pg_doorman.toml`
   - Подправьте настройки в `pg_doorman.toml` под ваше окружение.

4. **Запуск PgDoorman**:
   ```bash
   cargo run --release
   ```

5. **Запуск unit-тестов**:
   ```bash
   cargo test
   ```

## Интеграционное тестирование

PgDoorman использует BDD-тесты (Behavior-Driven Development) с тестовым окружением на Docker. **Воспроизводимость гарантирована** -- все тесты выполняются внутри Docker-контейнеров с одинаковым окружением.

### Тестовое окружение

Тестовый Docker-образ (собранный через Nix) включает:
- PostgreSQL 16
- Go 1.24
- Python 3 с asyncpg, psycopg2, aiopg, pytest
- Node.js 22
- .NET SDK 8
- Rust 1.87.0

### Запуск тестов

Из **корневой директории проекта**:

```bash
# Скачать тестовый образ из registry
make pull

# Или собрать локально (10-15 минут на первом запуске)
make local-build

# Запустить все BDD-тесты
make test-bdd

# Запустить тесты с конкретным тегом
make test-bdd TAGS=@copy-protocol
make test-bdd TAGS=@cancel
make test-bdd TAGS=@admin-commands

# Открыть интерактивный shell в тестовом контейнере
make shell
```

### Debug-режим

Включается переменной окружения `DEBUG=1`:

```bash
DEBUG=1 make test-bdd TAGS=@copy-protocol
```

Когда задан `DEBUG=1`:
- Включается tracing с уровнем DEBUG.
- В логах показываются ID потоков.
- Включается номер строки.
- Видны детали PostgreSQL-протокола.
- Логируется детальное пошаговое выполнение.

Это полезно, когда:
- Нужно отладить падающий тест.
- Хочется разобраться в коммуникации на уровне протокола.
- Расследуете проблемы с таймингами.
- Разрабатываете новые тестовые сценарии.

### Доступные теги тестов

| Тег | Описание |
|-----|----------|
| `@go` | Тесты Go-клиентов (lib/pq, pgx) |
| `@python` | Тесты Python-клиентов (asyncpg, psycopg2) |
| `@nodejs` | Тесты Node.js-клиентов (pg) |
| `@dotnet` | Тесты .NET-клиентов (Npgsql) |
| `@java` | Тесты Java-клиентов (JDBC) |
| `@php` | Тесты PHP-клиентов (PDO) |
| `@rust` | Тесты на уровне протокола, написанные на Rust |
| `@auth-query` | Тесты auth query authentication |
| `@copy-protocol` | Тесты COPY-протокола |
| `@cancel` | Тесты отмены запросов |
| `@admin-commands` | Команды admin-консоли |
| `@admin-leak` | Тесты на утечку admin-соединений |
| `@buffer-cleanup` | Тесты очистки буфера |
| `@rollback` | Тесты функциональности rollback |
| `@hba` | Тесты HBA-аутентификации |
| `@prometheus` | Тесты Prometheus-метрик |
| `@fuzz` | Fuzz-тесты на устойчивость |
| `@bench` | Замеры производительности |
| `@binary-upgrade-grac-shutdown` | Тесты binary upgrade и daemon-режима |
| `@static-passthrough` | Тесты static passthrough auth |

## Написание новых тестов

Тесты организованы как BDD-feature-файлы в `tests/bdd/features/`. Каждый feature-файл описывает тестовые сценарии в синтаксисе Gherkin.

### Shell-тесты (рекомендуются для клиентских библиотек)

Shell-тесты запускают внешние команды (Go, Python, Node.js, .NET, Java, PHP) и проверяют их вывод. Это самый простой способ протестировать совместимость с клиентской библиотекой.

**Пример** (`tests/bdd/features/my-feature.feature`):

```gherkin
@go @mytag
Feature: My feature description

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40
      """

  Scenario: Test my Go client
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run TestMyTest ./mypackage
      """
    Then the command should succeed
    And the command output should contain "PASS"
```

**Реализация теста** (на удобном вам языке):
- Go: `tests/go/mypackage/my_test.go`
- Python: `tests/python/test_my.py`
- Node.js: `tests/nodejs/my.test.js`
- .NET: `tests/dotnet/MyTest.cs`

### Тесты на уровне протокола на Rust

Чтобы тестировать поведение PostgreSQL-протокола на уровне сообщений, используйте Rust-тесты. Они напрямую отправляют и получают сообщения PostgreSQL-протокола, что даёт точный контроль и возможность сравнения.

**Пример** (`tests/bdd/features/protocol-test.feature`):

```gherkin
@rust @my-protocol-test
Feature: Protocol behavior test
  Testing that pg_doorman handles protocol messages identically to PostgreSQL

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  @my-scenario
  Scenario: Query gives identical results from PostgreSQL and pg_doorman
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to both
    Then we should receive identical messages from both

  @session-test
  Scenario: Session management test
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "one"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    # ... ещё шаги
```

**Доступные шаги Rust-тестов:**

Сравнение протоколов (отправляет и в PostgreSQL, и в pg_doorman):
- `we login to postgres and pg_doorman as "user" with password "pass" and database "db"`
- `we send SimpleQuery "SQL" to both`
- `we send CopyFromStdin "COPY ..." with data "..." to both`
- `we should receive identical messages from both`

Управление сессиями (для сложных сценариев):
- `we create session "name" to pg_doorman as "user" with password "pass" and database "db"`
- `we send SimpleQuery "SQL" to session "name"`
- `we send SimpleQuery "SQL" to session "name" and store backend_pid`
- `we abort TCP connection for session "name"`
- `we sleep 100ms`

Тестирование cancel-запросов:
- `we create session "name" ... and store backend key`
- `we send SimpleQuery "SQL" to session "name" without waiting for response`
- `we send cancel request for session "name"`
- `session "name" should receive cancel error containing "text"`

### Добавление зависимостей

Если в тестовом окружении нужны дополнительные пакеты, отредактируйте `tests/nix/flake.nix`:
- Python-пакеты добавляются в `pythonEnv`.
- Системные пакеты -- в `runtimePackages`.

После изменения `flake.nix` пересоберите образ командой `make local-build`.

## Правила вклада

### Стиль кода

- Следуйте Rust style guidelines.
- Используйте осмысленные имена переменных и функций.
- Добавляйте комментарии для нетривиальной логики.
- Пишите тесты для новой функциональности.

### Процесс Pull Request

1. **Создайте новую ветку** для своей фичи или багфикса.
2. **Внесите изменения** и закоммитьте их с понятными, описательными сообщениями.
3. **Напишите или обновите тесты**, если требуется.
4. **Обновите документацию**, отражая изменения.
5. **Откройте pull request** в основной репозиторий.
6. **Реагируйте на замечания** code review.

### Issues

Если нашли баг или хотите предложить новую функциональность, создайте issue в [репозитории на GitHub](https://github.com/ozontech/pg_doorman/issues) с:

- Чётким, описательным заголовком.
- Подробным описанием проблемы или фичи.
- Шагами воспроизведения (для багов).
- Ожидаемым и фактическим поведением (для багов).

## Где получить помощь

Если нужна помощь с вкладом в проект:

- Задавайте вопросы в [GitHub issues](https://github.com/ozontech/pg_doorman/issues).
- Заходите в Telegram-канал: [@pg_doorman](https://t.me/pg_doorman).
- Свяжитесь с maintainers.

Спасибо, что вносите вклад в PgDoorman!
