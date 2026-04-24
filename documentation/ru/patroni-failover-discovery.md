# Patroni failover discovery

Когда pg_doorman работает на одной машине с PostgreSQL и подключён
через unix socket, switchover в Patroni или аварийное падение PG
оставляют doorman без бэкенда. Все клиентские запросы получают ошибки,
пока DBA не перенастроит doorman или локальный PostgreSQL не поднимется.

Patroni failover discovery позволяет doorman перекрыть этот промежуток
автоматически. Когда локальный PostgreSQL перестаёт отвечать, doorman
запрашивает Patroni REST API, находит другой член кластера и направляет
новые соединения туда. Существующие соединения к мёртвому бэкенду
закрываются при штатном recycle.

Это краткосрочная мера. Она покрывает 10-30 секунд типичного switchover.
Долгосрочная маршрутизация (перенаправление doorman на нового master
навсегда) остаётся задачей DBA через reload конфигурации.

## Когда это помогает

**Плановый switchover.** DBA запускает `patroni switchover --candidate node2`.
Patroni промотирует node2, затем останавливает PostgreSQL на node1.
Между остановкой и обновлением конфига doorman на node1 не имеет бэкенда.
С включённым discovery doorman подключается к node2 за 1-2 TCP round trip.

**Аварийное падение.** PostgreSQL на node1 убит OOM killer. Patroni ещё
не обнаружил сбой. doorman получает connection refused на unix socket,
запрашивает Patroni API и подключается к `sync_standby` (вероятному
следующему лидеру).

## Когда это не помогает

**Падение машины.** Если вся машина недоступна, doorman мёртв вместе
с ней. Для этого сценария нужна внешняя маршрутизация (HAProxy,
patroni_proxy, DNS failover, VIP).

**Ошибки аутентификации.** Если PostgreSQL отклоняет credentials
doorman, бэкенд жив. Discovery не активируется.

## Как это работает

```
Штатный режим:
  клиент --unix--> doorman --unix--> PostgreSQL (локальный)

Fallback:
  клиент --unix--> doorman --TCP---> PostgreSQL (удалённый, из /cluster)
                      |
                      +-- GET /cluster --> Patroni API
```

1. `ServerPool::create()` пробует локальный unix socket.
2. Connection refused или socket error: doorman помещает локальный хост
   в blacklist на `failover_blacklist_duration_ms` (по умолчанию
   30 секунд).
3. doorman отправляет `GET /cluster` ко всем Patroni URL из конфига
   **параллельно** и берёт первый успешный ответ.
4. Из списка members doorman выбирает первый доступный хост:
   сначала `sync_standby`, потом `replica`, потом любой другой.
   TCP connect ко всем кандидатам запускается параллельно;
   `sync_standby` имеет приоритет (doorman ждёт до 2 секунд прежде
   чем взять replica).
5. Новое соединение попадает в пул со **сниженным lifetime**
   (по умолчанию 30 секунд, совпадает с длительностью blacklist).
   На него действуют все обычные правила пула: лимиты coordinator,
   idle timeout, recycle.
6. Последующие вызовы `create()` в рамках blacklist-окна подключаются
   к тому же fallback-хосту напрямую, без повторного запроса к
   Patroni API.
7. Когда blacklist истекает, doorman снова пробует локальный socket.
   Если работает -- штатный режим. Если нет -- цикл повторяется.

## Write-запросы на реплике

Если fallback-хост -- реплика, которая ещё не промотирована,
write-запросы получат ошибку от PostgreSQL:

```
ERROR: cannot execute INSERT in a read-only transaction
```

Read-запросы работают нормально. При типичном switchover `sync_standby`
промотируется раньше, чем doorman обнаруживает отказ, поэтому
большинство write-запросов проходит. В худшем случае write-ошибки
длятся до истечения сниженного lifetime (30 секунд), после чего
следующее соединение через свежий `/cluster` найдёт нового master.

## Конфигурация

Добавьте `patroni_discovery_urls` к любому пулу, который должен
использовать discovery. Без этого параметра фича полностью отключена,
doorman работает как раньше.

```yaml
pools:
  mydb:
    pool_mode: transaction
    server_host: "/var/run/postgresql"
    server_port: 5432

    # Адреса Patroni REST API. Укажите минимум 2 для отказоустойчивости.
    # Первый ответивший URL выигрывает; порядок не важен.
    patroni_discovery_urls:
      - "http://10.0.0.1:8008"
      - "http://10.0.0.2:8008"
      - "http://10.0.0.3:8008"
```

TOML-эквивалент:

```toml
[pools.mydb]
pool_mode = "transaction"
server_host = "/var/run/postgresql"
server_port = 5432

patroni_discovery_urls = [
    "http://10.0.0.1:8008",
    "http://10.0.0.2:8008",
    "http://10.0.0.3:8008",
]
```

### Параметры настройки

Все параметры опциональны и имеют разумные значения по умолчанию.

| Параметр | По умолчанию | Описание |
|----------|--------------|----------|
| `failover_blacklist_duration_ms` | 30000 | Сколько локальный хост остаётся в blacklist после ошибки соединения. В течение этого окна все новые соединения идут на fallback-хост. |
| `failover_discovery_timeout_ms` | 5000 | HTTP-таймаут запросов к Patroni API. Действует на каждый URL; так как все URL опрашиваются параллельно, реальный таймаут равен этому значению, а не умноженному на количество URL. |
| `failover_connect_timeout_ms` | 5000 | Таймаут TCP connect к fallback-серверам. Действует на всю пачку параллельных connect, не на каждый member отдельно. |
| `failover_server_lifetime_ms` | = `failover_blacklist_duration_ms` | Lifetime fallback-соединений. Короче штатного `server_lifetime`, чтобы doorman быстро вернулся к локальному хосту после завершения switchover. |

### Что указывать в `patroni_discovery_urls`

Перечислите адреса Patroni REST API ваших узлов кластера. Endpoint
`/cluster` на любом узле Patroni возвращает полную топологию кластера,
поэтому даже одного URL достаточно для обнаружения всех members.

Два и более URL рекомендуется: если первый URL указывает на ту же
машину что и мёртвый PostgreSQL, он тоже не ответит. doorman
опрашивает все URL параллельно и берёт первый ответ.

После успешного ответа `/cluster` doorman извлекает `api_url` из
каждого member и может использовать их для последующих запросов, даже
если URL из конфига стали неактуальными.

## Prometheus-метрики

| Метрика | Тип | Описание |
|---------|-----|----------|
| `pg_doorman_failover_discovery_total` | counter | Количество запросов `/cluster` |
| `pg_doorman_failover_connections_total` | counter | Создано fallback-соединений |
| `pg_doorman_failover_discovery_errors_total` | counter | Неудачные запросы `/cluster` (все URL недоступны) |
| `pg_doorman_failover_host_blacklisted` | gauge | 1, если основной хост в blacklist |
| `pg_doorman_failover_discovery_duration_seconds` | histogram | Время запроса `/cluster` |

## Связь с patroni_proxy

patroni_proxy и failover discovery решают разные задачи.

**patroni_proxy** -- TCP-балансировщик, разворачивается рядом с
клиентскими приложениями. Маршрутизирует соединения к нужному узлу
PostgreSQL по роли (leader, sync, async). Не пулит соединения.

**Failover discovery** -- встроен в pooler doorman, который
разворачивается рядом с PostgreSQL. Обрабатывает ситуацию, когда
локальный бэкенд умер и doorman нуждается во временной альтернативе.
Пулит соединения.

В рекомендуемой архитектуре (patroni_proxy -> pg_doorman -> PostgreSQL)
failover discovery добавляет устойчивость на уровне doorman, не
затрагивая уровень patroni_proxy.
