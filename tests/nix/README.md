# pg_doorman Multi-Language Test Environment

Nix-based Docker image для тестирования pg_doorman с клиентами на Go, Python, Ruby, Node.js, и .NET.

## 🚀 Быстрый старт (БЕЗ локального Nix!)

Вам нужен только **Docker**. Nix не требуется локально!

### 1. Скачать готовый образ

```bash
cd tests/nix

# Скачать latest образ
make pull

# Или вручную
./run-tests.sh pull
```

### 2. Запустить тесты

```bash
# Интерактивный shell
make shell

# Собрать pg_doorman
make build

# Запустить BDD тесты
make test-bdd

# Запустить тесты конкретного языка
make test-go
make test-python
make test-ruby
make test-nodejs
make test-dotnet

# Все тесты сразу
make test-all
```

## 📋 Доступные команды

### Makefile команды

```bash
make help           # Показать все доступные команды
make pull           # Скачать образ из registry
make shell          # Открыть bash в контейнере
make build          # Собрать pg_doorman внутри контейнера

# Запуск тестов
make test-bdd       # Cucumber/BDD тесты
make test-bdd TAGS=@go      # Только тесты с тегом @go
make test-go        # Go тесты
make test-python    # Python тесты
make test-ruby      # Ruby тесты
make test-nodejs    # Node.js тесты
make test-dotnet    # .NET тесты
make test-all       # Все языковые тесты

make clean          # Очистить Docker volumes кэша
```

### Использование run-tests.sh напрямую

```bash
./run-tests.sh pull                 # Скачать образ
./run-tests.sh shell                # Интерактивный shell
./run-tests.sh build                # Собрать pg_doorman
./run-tests.sh bdd                  # BDD тесты
./run-tests.sh bdd @go              # BDD тесты только для Go
./run-tests.sh test-python          # Python тесты
```

## 🎯 Локальная разработка

### Интерактивная сессия

```bash
cd tests/nix
make shell

# Внутри контейнера:
setup-test-deps                     # Подготовить зависимости
cargo build --release               # Собрать pg_doorman
cargo test --test bdd               # Запустить BDD тесты
cd tests/go && go test -v .         # Go тесты
```

### Примеры workflow

```bash
# 1. Скачать образ и запустить shell
make pull shell

# 2. Внутри контейнера собрать и протестировать
cargo build --release
cargo test --test bdd -- --tags @go

# 3. Запустить конкретные Go тесты
cd tests/go
go test -v -run TestExtendedProtocol
```

## ⚡ Кэширование для максимальной скорости

Образ использует несколько уровней кэширования:

### 1. **Docker Layer Caching** (через Nix)
- Base system (coreutils, bash) - почти никогда не меняется
- Языковые runtime (PostgreSQL, Go, Python, Ruby, Node.js, .NET) - меняется при смене версий
- Build dependencies (gcc, pkg-config, openssl) - почти никогда не меняется
- **Pre-cached зависимости** (Go modules, Ruby gems, npm packages, Cargo crates) - меняется при изменении lock-файлов
- Helper scripts - меняется редко, но маленький размер

### 2. **Persistent Docker Volumes**

При запуске тестов создаются named volumes для кэширования:
- `pg_doorman_cargo_cache` - Rust crates registry
- `pg_doorman_go_cache` - Go modules
- `pg_doorman_ruby_gems` - Ruby gems
- `pg_doorman_npm_cache` - npm packages
- `pg_doorman_dotnet` - .NET packages

**Первый запуск:** ~5-10 минут (загрузка зависимостей)
**Последующие запуски:** ~30 секунд (используются volumes)

### 3. **Очистка кэша**

```bash
make clean          # Удалить все volume кэши
```

## 🔧 Переменные окружения

```bash
# Registry настройки
export REGISTRY=ghcr.io                         # Container registry
export IMAGE_TAG=latest                         # Тег образа (latest, main, pr-123)

# Использование
make pull
```

## 🏗️ Архитектура образа

```
pg_doorman-test-env:latest
├── /cache/                     # Pre-cached dependencies
│   ├── go/pkg/mod/            # Go modules
│   ├── ruby/bundle/           # Ruby gems
│   ├── node/node_modules/     # npm packages
│   └── cargo/                 # Rust crates
├── /bin/                      # Все языковые runtime
│   ├── postgres               # PostgreSQL 16
│   ├── go                     # Go 1.24
│   ├── python3                # Python 3.x
│   ├── ruby                   # Ruby 3.3
│   ├── node                   # Node.js 22
│   ├── dotnet                 # .NET SDK 8
│   ├── rustc/cargo            # Rust 1.87
│   └── setup-test-deps        # Helper для линковки кэшей
└── /workspace/                # Mount point для проекта
```

## 🐛 Отладка

### Проверить размер образа

```bash
docker images pg_doorman-test-env
```

### Проверить кэшированные зависимости

```bash
make shell

# Внутри контейнера:
ls -lh /cache/go/pkg/mod/       # Go modules
ls -lh /cache/ruby/bundle/      # Ruby gems
ls -lh /cache/node/node_modules/ # npm packages
```

### Проверить version языков

```bash
make shell

# Внутри контейнера:
postgres --version
go version
python3 --version
ruby --version
node --version
dotnet --version
rustc --version
```

## 📦 CI/CD Integration

Образ автоматически собирается и публикуется в GHCR на каждый PR:

```yaml
# В вашем workflow:
- name: Pull test image
  run: |
    docker pull ghcr.io/${{ github.repository }}/test-runner:latest

- name: Run tests
  run: |
    cd tests/nix
    make test-all
```

## 🤔 FAQ

### Нужен ли мне локальный Nix?

**Нет!** Образ собирается в CI с помощью Nix, но локально вы просто используете Docker.

### Как обновить образ?

```bash
make pull   # Скачает latest версию
```

### Почему первый запуск медленный?

Docker качает все layers образа (~2-5 GB). Последующие запуски используют локальный кэш.

### Можно ли использовать свой registry?

```bash
export REGISTRY=my-registry.com
export IMAGE_TAG=my-tag
make pull
```

### Как добавить новые зависимости?

1. Обновите `flake.nix` (добавьте в pre-cache секцию)
2. Пересоберите образ в CI
3. `make pull` для получения обновленного образа

## 📚 Дополнительно

- **Размер образа:** ~3-5 GB (с pre-cached зависимостями)
- **Поддерживаемые архитектуры:** x86_64-linux, aarch64-linux (через Nix)
- **Base:** NixOS minimal (не Debian/Alpine!)
- **Layer count:** до 125 layers для максимального кэширования
