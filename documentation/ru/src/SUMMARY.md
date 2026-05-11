# Содержание

[Главная](index.md)
[Сравнение](comparison.md)

---

# Начало работы

- [Обзор](tutorials/overview.md)
- [Установка](tutorials/installation.md)
- [Базовое использование](tutorials/basic-usage.md)

# Аутентификация

- [Обзор](authentication/overview.md)
- [Passthrough-аутентификация (по умолчанию)](authentication/passthrough.md)
- [auth_query](authentication/auth-query.md)
- [PAM](authentication/pam.md)
- [JWT](authentication/jwt.md)
- [Talos](authentication/talos.md)
- [pg_hba.conf](authentication/hba.md)

# TLS

- [Клиентский и серверный TLS](guides/tls.md)

# Пулинг

- [Режимы пула](concepts/pool-modes.md)
- [Координатор пулов](concepts/pool-coordinator.md)
- [Кеш Parse для анонимных prepared statements](tutorials/prepared-statements.md)
- [Startup-параметры PostgreSQL](tutorials/startup-parameters.md)
- [Пул под нагрузкой (продвинутое)](tutorials/pool-pressure.md)

# Высокая доступность

- [Fallback через Patroni](tutorials/patroni-assisted-fallback.md)
- [patroni_proxy](tutorials/patroni-proxy.md)

# Эксплуатация

- [Плавное обновление бинаря](tutorials/binary-upgrade.md)
- [Сигналы и перезагрузка](operations/signals.md)
- [Мониторинг query interner](operations/monitoring-interner.md)
- [Диагностика](tutorials/troubleshooting.md)

# Мониторинг и диагностика

- [Команды администратора](observability/admin-commands.md)
- [Веб-консоль](guides/web-ui.md)
- [Структурированное JSON-логирование](observability/json-logging.md)
- [Перцентили задержек](observability/percentiles.md)

# Справочник

- [Общие настройки](reference/general.md)
- [Настройки пула](reference/pool.md)
- [Метрики Prometheus](reference/prometheus.md)

---

- [Бенчмарки](benchmarks.md)
- [История изменений](changelog.md)
- [Участие в проекте](tutorials/contributing.md)
