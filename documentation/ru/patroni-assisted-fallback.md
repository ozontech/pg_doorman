# Patroni-assisted fallback

Когда pg_doorman работает на одной машине с PostgreSQL и подключён
через unix socket, switchover в Patroni или аварийное падение PG
оставляют doorman без бэкенда. Пока Patroni не закончит promote реплики
или не перезапустит локальный PostgreSQL, все клиентские запросы падают.

Patroni-assisted fallback перекрывает этот промежуток. Когда локальный
PostgreSQL перестаёт отвечать, pg_doorman запрашивает Patroni REST API,
выбирает другого члена кластера и направляет новые соединения туда.
Существующие соединения к мёртвому бэкенду закрываются при штатном recycle.

Это краткосрочная мера. Она перекрывает 10-30 секунд, пока Patroni
завершает свой failover. Когда Patroni восстановит локальный PostgreSQL
(как реплику нового primary или как восстановленный primary), pg_doorman
сам вернётся к локальному socket.

## Когда это помогает

**Плановый switchover.** DBA запускает `patroni switchover --candidate node2`.
Patroni промотирует node2, затем останавливает PostgreSQL на node1.
Между остановкой и тем, как Patroni перезапустит node1 как реплику node2,
doorman на node1 не имеет бэкенда. С включённым fallback doorman
подключается к node2 за 1-2 TCP round trip.

**Аварийное падение.** PostgreSQL на node1 убит OOM killer. Patroni ещё
не обнаружил сбой. doorman получает connection refused на unix socket,
запрашивает Patroni API и подключается к `sync_standby` (вероятному
следующему лидеру).

## Когда это не помогает

**Падение машины.** Если вся машина недоступна, doorman мёртв вместе
с ней. Для этого сценария нужна внешняя маршрутизация (HAProxy,
patroni_proxy, DNS failover, VIP).

**Ошибки аутентификации.** Если PostgreSQL отклоняет credentials
doorman, бэкенд жив. Fallback не активируется.

## Как это работает

```
Штатный режим:
  клиент --unix--> doorman --unix--> PostgreSQL (локальный)

Fallback:
  клиент --unix--> doorman --TCP---> PostgreSQL (удалённый, из /cluster)
                      |
                      +-- GET /cluster --> Patroni API
```

1. Doorman пробует локальный unix socket.
2. Connection refused или socket error: doorman помечает локальный
   backend как недоступный на `fallback_cooldown` (по умолчанию 30 секунд).
3. doorman отправляет `GET /cluster` ко всем Patroni URL из конфига
   **параллельно** и берёт первый успешный ответ.
4. Из списка members doorman выбирает кандидата с наивысшим приоритетом:
   сначала `sync_standby`, потом `replica`, потом любой другой.
   TCP connect ко всем кандидатам запускается параллельно; если
   `sync_standby` отвечает, он выбирается немедленно, обходя replica.
5. Новое соединение попадает в пул со **сниженным lifetime**
   (по умолчанию 30 секунд, совпадает с `fallback_cooldown`).
   На него действуют все обычные правила пула: лимиты coordinator,
   idle timeout, recycle.
6. Последующие соединения в рамках cooldown идут к тому же
   fallback-хосту напрямую, без повторного запроса к Patroni API.
7. Когда cooldown истекает, doorman снова пробует локальный socket.
   Если работает — штатный режим. Если нет — цикл повторяется.

## Write-запросы на реплике

Если fallback-хост — реплика, которая ещё не промотирована,
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

Добавьте `patroni_api_urls` к любому пулу, который должен
использовать fallback. Без этого параметра фича отключена,
doorman работает как раньше.

```yaml
pools:
  mydb:
    pool_mode: transaction
    server_host: "/var/run/postgresql"
    server_port: 5432

    # Адреса Patroni REST API. Укажите минимум 2 для отказоустойчивости.
    # Первый ответивший URL выигрывает; порядок не важен.
    patroni_api_urls:
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

patroni_api_urls = [
    "http://10.0.0.1:8008",
    "http://10.0.0.2:8008",
    "http://10.0.0.3:8008",
]
```

### Параметры настройки

Все параметры опциональны и имеют разумные значения по умолчанию.

| Параметр | По умолчанию | Описание |
|----------|--------------|----------|
| `fallback_cooldown` | `"30s"` | Сколько локальный backend остаётся помеченным как недоступный после ошибки соединения. В течение этого окна все новые соединения идут на fallback-хост. |
| `patroni_api_timeout` | `"5s"` | HTTP-таймаут запросов к Patroni API. Действует на каждый URL; так как все URL опрашиваются параллельно, реальный таймаут равен этому значению, а не умноженному на количество URL. |
| `fallback_connect_timeout` | `"5s"` | Таймаут TCP connect к fallback-кандидатам. Действует на всю пачку параллельных connect, не на каждый member отдельно. |
| `fallback_lifetime` | = `fallback_cooldown` | Lifetime fallback-соединений. Короче штатного `server_lifetime`, чтобы doorman быстро вернулся к локальному backend после восстановления. |

### Что указывать в `patroni_api_urls`

Перечислите адреса Patroni REST API ваших узлов кластера. Endpoint
`/cluster` на любом узле Patroni возвращает полную топологию кластера,
поэтому даже одного URL достаточно для перечисления всех members.

Два и более URL рекомендуется: если первый URL указывает на ту же
машину что и мёртвый PostgreSQL, он тоже не ответит. doorman
опрашивает все URL параллельно и берёт первый ответ.

## Prometheus-метрики

| Метрика | Тип | Описание |
|---------|-----|----------|
| `pg_doorman_patroni_api_requests_total` | counter | Количество запросов `/cluster` |
| `pg_doorman_fallback_connections_total` | counter | Создано fallback-соединений |
| `pg_doorman_patroni_api_errors_total` | counter | Неудачные запросы `/cluster` (все URL недоступны) |
| `pg_doorman_fallback_active` | gauge | 1, пока локальный backend в cooldown и пул использует fallback |
| `pg_doorman_fallback_host` | gauge | Текущий активный fallback-хост (1 = активен). Labels: pool, host, port |
| `pg_doorman_fallback_cache_hits_total` | counter | Повторное использование кешированного fallback-хоста без запроса к Patroni API |
| `pg_doorman_patroni_api_duration_seconds` | histogram | Время запроса `/cluster` |

## Активные транзакции

Если PostgreSQL падает во время транзакции клиента, клиент получает
ошибку соединения. doorman не переносит незавершённые транзакции
на fallback-хост — клиент должен выполнить retry.

Новые запросы от этого и других клиентов автоматически идут через
fallback.

## Эксплуатационные заметки

**Credentials.** Все узлы кластера должны принимать те же username
и password, которые использует doorman. Patroni-кластеры обычно
разделяют `pg_hba.conf` через bootstrap-конфигурацию, но это не
гарантировано. Убедитесь, что fallback-узлы принимают настроенные
credentials.

**TLS.** Fallback-соединения используют тот же `server_tls_mode`,
что и локальный backend. Если локальный backend идёт через unix
socket (без TLS), fallback TCP-соединения тоже пойдут без TLS.
Настройте `server_tls_mode` явно, если fallback-соединения должны
быть зашифрованы.

**DNS.** Используйте IP-адреса в `patroni_api_urls`, а не hostname.
Неудача DNS-резолва во время failover добавляет задержку и может
привести к полному отказу запроса.

**standby_leader.** В standby-кластерах Patroni используется роль
`standby_leader`. doorman обрабатывает её как «other» (наименьший
приоритет, после sync_standby и replica). Для большинства
развёртываний это корректно.

## Связь с patroni_proxy

patroni_proxy и Patroni-assisted fallback решают разные задачи.

**patroni_proxy** — TCP-балансировщик, разворачивается рядом с
клиентскими приложениями. Маршрутизирует соединения к нужному узлу
PostgreSQL по роли (leader, sync, async). Не пулит соединения.

**Patroni-assisted fallback** — встроен в pooler doorman, который
разворачивается рядом с PostgreSQL. Обрабатывает ситуацию, когда
локальный backend умер и doorman нуждается во временной альтернативе.
Пулит соединения.

В рекомендуемой архитектуре (patroni_proxy → pg_doorman → PostgreSQL)
fallback сохраняет read-трафик на уровне doorman при падении
локального backend, не затрагивая маршрутизацию patroni_proxy.
