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

## Быстрый старт

Рекомендуемая схема — pg_doorman рядом с PostgreSQL на одной машине,
ходит к нему через unix socket. Patroni REST API тоже на `localhost`,
поэтому fallback включается одной строкой в `[general]`:

```yaml
general:
  patroni_api_urls: ["http://localhost:8008"]
```

Каждый пул подхватывает это автоматически. Когда unix socket перестаёт
отвечать, pg_doorman запрашивает `/cluster`, выбирает кандидата по
приоритету `sync_standby` > `replica` > leader и направляет новые
соединения на выбранный хост, пока локальный PostgreSQL не вернётся в
строй. Значения по умолчанию: cooldown 30s, HTTP-таймаут 5s, TCP-таймаут
5s, lifetime fallback-соединений 30s. Переопределить их можно через
[параметры настройки](#параметры-настройки).

## Когда это помогает

**Плановый switchover.** DBA запускает `patroni switchover --candidate node2`.
Patroni промотирует node2, затем останавливает PostgreSQL на node1.
Между остановкой и тем, как Patroni перезапустит node1 как реплику node2,
doorman на node1 не имеет бэкенда. С включённым fallback следующий
клиентский запрос, не сумевший дойти до локального сокета, триггерит
`/cluster` lookup, и новое соединение открывается к node2.

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

1. doorman пробует локальный unix socket.
2. Connection refused или socket error: doorman помечает локальный
   backend как недоступный на `fallback_cooldown` (по умолчанию 30 секунд).
3. doorman отправляет `GET /cluster` ко всем Patroni URL из конфига
   **параллельно** и берёт первый успешный ответ.
4. Из списка members doorman отбрасывает находящихся в cooldown и
   делит остальных на две волны по роли: волна 1 — все
   `sync_standby`; волна 2 — все остальные (replica + leader, в
   порядке discovery).
5. **Волна 1 (строгий приоритет sync_standby).** doorman параллельно
   запускает `Server::startup` для каждого sync_standby, каждый под
   `fallback_connect_timeout` (по умолчанию 5 секунд). Первый
   sync_standby, успешно прошедший startup, выигрывает и его
   соединение отдаётся клиенту. Пока хоть один sync_standby ещё в
   процессе startup, replica/leader не учитываются — даже если
   replica уже готова. Цель — сохранить пишущий трафик: sync_standby
   это кандидат на promote с минимальной потерей данных.
6. **Волна 2 (первый успех).** Запускается, только если все
   sync_standby упали (или их нет в кластере). doorman параллельно
   стартует остальных кандидатов под тем же per-candidate timeout;
   первый успех выигрывает.
7. **Exhaustion.** Если обе волны кончились без победителя, в лог
   doorman пишется
   `all fallback candidates rejected (3 startup_error, 1 timeout)` с
   детерминированной разбивкой по причинам. Клиент получает
   sanitized FATAL — `Unable to retrieve server parameters … may be
   unavailable or misconfigured`; разбор смотрите в логе doorman.
8. Успешное соединение попадает в пул со **сниженным lifetime**
   (по умолчанию 30 секунд, совпадает с `fallback_cooldown`).
   На него действуют все обычные правила пула: лимиты coordinator,
   idle timeout, recycle.
9. Последующие соединения в рамках cooldown идут к тому же
   fallback-хосту напрямую, без повторного запроса к Patroni API.
   Если кэшированный host позже отказывает на startup, doorman
   очищает кэш и выполняет один дополнительный раунд discovery.
10. Когда cooldown истекает, doorman снова пробует локальный socket.
    Если работает — штатный режим. Если нет — цикл повторяется.

Per-candidate отказ (auth, `database is starting up`, таймаут) ставит
кандидата в cooldown с экспоненциальным backoff; следующие раунды
discovery пропускают эти хосты до истечения окна.

### Ограничение времени ожидания клиента

Клиент никогда не ждёт fallback дольше `query_wait_timeout`
(по умолчанию 5 секунд). Если дедлайн срабатывает, doorman прерывает
fallback с записью `fallback: outer deadline {ms}ms exceeded` в лог,
а клиент получает sanitized FATAL — тот же, что и на любую другую
startup-time ошибку. Дедлайн **мягкий**: жёсткую защиту от зависаний
обеспечивает per-candidate `fallback_connect_timeout`, а внешний
дедлайн — это верхняя граница того, сколько клиент сам готов ждать.

### Cooldown на отдельных хостах

Кандидат, отказавший на startup, исключается из ближайшего discovery
на `fallback_connect_timeout` (по умолчанию 5 секунд). Каждый
последовательный отказ того же хоста удваивает cooldown, с верхним
пределом 60 секунд. После окончания окна запись удаляется (lazy
cleanup на следующем discovery-цикле), счётчик сбрасывается на
следующем отказе. Это не даёт застрявшему кандидату (postgres в
recovery, persistent ошибка auth, медленная сеть) повторно
тестироваться на каждый клиентский запрос и заваливать одновременно
и кандидата, и Patroni API.

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
| `fallback_connect_timeout` | `"5s"` | Дедлайн `Server::startup` для каждого кандидата (покрывает TCP-connect плюс StartupMessage round-trip) и база per-host cooldown после отказа startup. Один параметр на две роли — обе имеют одинаковую семантику «кандидат не отвечает». |
| `fallback_lifetime` | = `fallback_cooldown` | Lifetime fallback-соединений. Короче штатного `server_lifetime`, чтобы doorman быстро вернулся к локальному backend после восстановления. |
| `connect_timeout` (`[general]`) | `"3s"` | Дедлайн `Server::startup` для локального backend, в дополнение к существующим ролям для alive-check и TCP probe. Поднимите этот параметр, если ваш локальный PostgreSQL имеет медленный startup (большой WAL replay, прогрев `shared_buffers`). |
| `query_wait_timeout` (`[general]`) | `"5s"` | Внешний дедлайн всего fallback-пути. Клиент никогда не ждёт server connection дольше этого значения, независимо от количества перебираемых кандидатов. |

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
| `pg_doorman_fallback_candidate_failures_total` | counter | Отказ конкретного кандидата на startup. Labels: `pool`, `reason` (`connect_error`, `startup_error`, `server_unavailable`, `timeout`, `other`). По разбивке по reason при exhaustion видно, что произошло — auth-фейлы на всех нодах или сетевая проблема. |
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

**DNS.** Используйте IP-адреса и в `patroni_api_urls`, и в Patroni
`member.host`, не hostname. Дедлайн startup покрывает DNS-резолв
через `TcpStream::connect`, но 5-секундный DNS hang съест весь
бюджет `fallback_connect_timeout` для конкретного кандидата прежде
чем будет проверен следующий.

**Объём логов при failure storm.** Per-candidate
`<host>:<port> rejected (...)` WARN ограничен одной строкой в
10 секунд на `(pool, host, port)`. Подавленные строки уходят в
DEBUG. Если вы видите один WARN там, где ожидали много — это
rate-limit, не потерянные данные; проверяйте счётчик
`pg_doorman_fallback_candidate_failures_total` для реального
количества попыток.

**Switchover whitelist и `pg_doorman_fallback_host`.** Когда
fallback-цель меняется (cooldown истёк, retry-раунд выбрал другой
host), gauge для предыдущего `(host, port)` удаляется в той же
операции, что устанавливает gauge для нового. Дашборды не видят
два хоста одновременно помеченных как активные во время
переключения.

**standby_leader.** В standby-кластерах Patroni используется роль
`standby_leader`. doorman обрабатывает её как «other» (наименьший
приоритет, после sync_standby и replica). Для primary-кластерного
развёртывания это то, что нужно; если же pg_doorman работает на
standby-кластере, fallback вам, скорее всего, не нужен вообще —
писать всё равно некуда.

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
