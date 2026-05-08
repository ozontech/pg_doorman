# Fallback через Patroni

Когда pg_doorman работает на одной машине с PostgreSQL и подключён
через Unix-сокет, switchover в Patroni или аварийное падение PostgreSQL
оставляют doorman без локального сервера. Пока Patroni не закончит promote реплики
или не перезапустит локальный PostgreSQL, все клиентские запросы падают.

Fallback через Patroni перекрывает этот промежуток. Когда локальный
PostgreSQL перестаёт отвечать, pg_doorman запрашивает Patroni REST API,
выбирает другого члена кластера и направляет новые соединения туда.
Существующие соединения к мёртвому серверу закрываются при штатной ротации.

Это краткосрочная мера. Она перекрывает 10-30 секунд, пока Patroni
завершает свой failover. Когда Patroni восстановит локальный PostgreSQL
(как реплику нового primary или как восстановленный primary), pg_doorman
сам вернётся к локальному сокету.

## Быстрый старт

Рекомендуемая схема — pg_doorman рядом с PostgreSQL на одной машине,
ходит к нему через Unix-сокет. Patroni REST API тоже на `localhost`,
поэтому fallback включается одной строкой в `[general]`:

```yaml
general:
  patroni_api_urls: ["http://localhost:8008"]
```

Каждый пул подхватывает это автоматически. Когда Unix-сокет перестаёт
отвечать, pg_doorman запрашивает `/cluster`, выбирает кандидата по
приоритету `sync_standby` > `replica` > leader и направляет новые
соединения на выбранный хост, пока локальный PostgreSQL не вернётся в
строй. Значения по умолчанию: период охлаждения 30s, HTTP-таймаут 5s, TCP-таймаут
5s, lifetime fallback-соединений 30s. Переопределить их можно через
[параметры настройки](#параметры-настройки).

## Когда это помогает

**Плановый switchover.** DBA запускает `patroni switchover --candidate node2`.
Patroni промотирует node2, затем останавливает PostgreSQL на node1.
Между остановкой и тем, как Patroni перезапустит node1 как реплику node2,
doorman на node1 не имеет локального сервера. С включённым fallback следующий
клиентский запрос, не сумевший дойти до локального сокета, триггерит
`/cluster` lookup, и новое соединение открывается к node2.

**Аварийное падение.** PostgreSQL на node1 убит OOM killer. Patroni ещё
не обнаружил сбой. doorman получает `connection refused` на Unix-сокете,
запрашивает Patroni API и подключается к `sync_standby` (вероятному
следующему лидеру).

## Когда это не помогает

**Падение машины.** Если вся машина недоступна, doorman мёртв вместе
с ней. Для этого сценария нужна внешняя маршрутизация (HAProxy,
patroni_proxy, DNS failover, VIP).

**Ошибки аутентификации.** Если PostgreSQL отклоняет учётные данные
doorman, сервер жив. Fallback не активируется.

## Как это работает

```
Штатный режим:
  клиент --unix--> doorman --unix--> PostgreSQL (локальный)

Fallback:
  клиент --unix--> doorman --TCP---> PostgreSQL (удалённый, из /cluster)
                      |
                      +-- GET /cluster --> Patroni API
```

1. doorman пробует локальный Unix-сокет.
2. `Connection refused` или ошибка сокета: doorman помечает локальный
   сервер как недоступный на `fallback_cooldown` (по умолчанию 30 секунд).
3. doorman отправляет `GET /cluster` ко всем Patroni URL из конфига
   **параллельно** и берёт первый успешный ответ.
4. Из списка участников doorman отбрасывает находящихся в периоде охлаждения и
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
6. **Волна 2 (без приоритета).** Запускается, только если все
   sync_standby упали (или их нет в кластере). doorman параллельно
   стартует остальных кандидатов под тем же per-candidate timeout;
   выигрывает первый, кто успешно завершит startup — replica и
   leader идут на равных.
7. **Все кандидаты исчерпаны.** Если обе волны кончились без победителя, в лог
   doorman пишется
   `all fallback candidates rejected (3 startup_error, 1 timeout)` с
   детерминированной разбивкой по причинам. Клиент получает
   очищенную FATAL-ошибку — `Unable to retrieve server parameters … may be
   unavailable or misconfigured`; разбор смотрите в логе doorman.
8. Успешное соединение попадает в пул со **сниженным lifetime**
   (по умолчанию 30 секунд, совпадает с `fallback_cooldown`).
   На него действуют все обычные правила пула: лимиты координатора,
   idle timeout, ротация.
9. Последующие соединения в рамках периода охлаждения идут к тому же
   fallback-хосту напрямую, без повторного запроса к Patroni API.
   Если кэшированный host позже отказывает на startup, doorman
   очищает кэш и выполняет один дополнительный раунд discovery.
10. Когда период охлаждения истекает, doorman снова пробует локальный сокет.
    Если работает — штатный режим. Если нет — цикл повторяется.

Отказ отдельного кандидата (auth, `database is starting up`, таймаут) ставит
кандидата в период охлаждения с экспоненциальным backoff; следующие раунды
discovery пропускают эти хосты до истечения окна.

### Ограничение времени ожидания клиента

Клиент никогда не ждёт fallback дольше `query_wait_timeout`
(по умолчанию 5 секунд). Если дедлайн срабатывает, doorman прерывает
fallback с записью `fallback: outer deadline {ms}ms exceeded` в лог,
а клиент получает очищенную FATAL-ошибку — ту же, что и на любую другую
startup-time ошибку. Дедлайн **мягкий**: жёсткую защиту от зависаний
обеспечивает per-candidate `fallback_connect_timeout`, а внешний
дедлайн — это верхняя граница того, сколько клиент сам готов ждать.

### Период охлаждения для отдельных хостов

Кандидат, отказавший на startup, исключается из ближайшего discovery
на `fallback_connect_timeout` (по умолчанию 5 секунд). Каждый
последовательный отказ того же хоста удваивает период охлаждения, с верхним
пределом 60 секунд. После окончания окна запись удаляется (lazy
cleanup на следующем discovery-цикле), счётчик сбрасывается на
следующем отказе. Это не даёт застрявшему кандидату (postgres в
recovery, постоянная ошибка auth, медленная сеть) повторно
тестироваться на каждый клиентский запрос и заваливать одновременно
и кандидата, и Patroni API.

## Запись на реплике

Если fallback-хост — реплика, которая ещё не промотирована,
запросы на запись получат ошибку от PostgreSQL:

```
ERROR: cannot execute INSERT in a read-only transaction
```

Запросы на чтение работают нормально. При типичном switchover `sync_standby`
промотируется раньше, чем doorman обнаруживает отказ, поэтому
большинство запросов на запись проходит. В худшем случае ошибки записи
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
| `fallback_cooldown` | `"30s"` | Сколько локальный сервер остаётся помеченным как недоступный после ошибки соединения. В течение этого окна все новые соединения идут на fallback-хост. |
| `patroni_api_timeout` | `"5s"` | HTTP-таймаут запросов к Patroni API. Действует на каждый URL; так как все URL опрашиваются параллельно, реальный таймаут равен этому значению, а не умноженному на количество URL. |
| `fallback_connect_timeout` | `"5s"` | Дедлайн `Server::startup` для каждого кандидата (покрывает TCP-connect плюс StartupMessage round-trip) и база per-host cooldown после отказа startup. Один параметр на две роли — обе имеют одинаковую семантику «кандидат не отвечает». |
| `fallback_lifetime` | = `fallback_cooldown` | Время жизни fallback-соединений. Короче штатного `server_lifetime`, чтобы doorman быстро вернулся к локальному серверу после восстановления. |
| `connect_timeout` (`[general]`) | `"3s"` | Дедлайн `Server::startup` для локального сервера, в дополнение к существующим ролям для alive-check и TCP probe. Поднимите этот параметр, если ваш локальный PostgreSQL имеет медленный startup (большой WAL replay, прогрев `shared_buffers`). |
| `query_wait_timeout` (`[general]`) | `"5s"` | Внешний дедлайн всего fallback-пути. Клиент никогда не ждёт серверное соединение дольше этого значения, независимо от количества перебираемых кандидатов. |

### Что указывать в `patroni_api_urls`

Перечислите адреса Patroni REST API ваших узлов кластера. Эндпоинт
`/cluster` на любом узле Patroni возвращает полную топологию кластера,
поэтому даже одного URL достаточно для перечисления всех участников.

Два и более URL рекомендуется: если первый URL указывает на ту же
машину что и мёртвый PostgreSQL, он тоже не ответит. doorman
опрашивает все URL параллельно и берёт первый ответ.

## Prometheus-метрики

| Метрика | Тип | Описание |
|---------|-----|----------|
| `pg_doorman_patroni_api_requests_total` | counter | Количество запросов `/cluster` |
| `pg_doorman_fallback_connections_total` | counter | Создано fallback-соединений |
| `pg_doorman_patroni_api_errors_total` | counter | Неудачные запросы `/cluster` (все URL недоступны) |
| `pg_doorman_fallback_active` | gauge | 1, пока локальный сервер в периоде охлаждения и пул использует fallback |
| `pg_doorman_fallback_host` | gauge | Текущий активный fallback-хост (1 = активен). Лейблы: pool, host, port |
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

patroni_proxy и fallback через Patroni решают разные задачи.

**patroni_proxy** — TCP-балансировщик, разворачивается рядом с
клиентскими приложениями. Маршрутизирует соединения к нужному узлу
PostgreSQL по роли (leader, sync, async). Пулинг соединений не выполняет.

**Fallback через Patroni** — встроен в pooler doorman, который
разворачивается рядом с PostgreSQL. Обрабатывает ситуацию, когда
локальный backend умер и doorman нуждается во временной альтернативе.
Пулинг соединений выполняет.

В рекомендуемой архитектуре (patroni_proxy → pg_doorman → PostgreSQL)
fallback сохраняет read-трафик на уровне doorman при падении
локального backend, не затрагивая маршрутизацию patroni_proxy.
