# Fastpath и large objects

Используйте эту страницу, если pgjdbc или Hibernate работают с PostgreSQL large
objects через pg_doorman в режиме transaction pool.

pgjdbc `LargeObjectManager` вызывает функции PostgreSQL large object через
Fastpath `FunctionCall` (`F`): `lo_creat`, `lo_open`, `lo_read`, `lo_write`,
`lo_close` и другие. PostgreSQL отвечает `FunctionCallResponse` (`V`), а затем
`ReadyForQuery` (`Z`). В `V` лежит результат функции. Состояние транзакции
приходит в следующем `ReadyForQuery`.

До 3.10.7 pg_doorman не передавал `FunctionCall` в transaction pooling. Клиент
мог отправить вызов функции large object и навсегда ждать ответа. Начиная с
3.10.7 pg_doorman передаёт вызов в PostgreSQL, возвращает клиенту
`FunctionCallResponse` и освобождает backend только после `ReadyForQuery`, если
PostgreSQL сообщил idle-состояние.

## Transaction pooling

Дескрипторы large object живут внутри PostgreSQL-транзакции. Если после
fastpath-вызова `ReadyForQuery` вернул статус `T` или `E`, pg_doorman оставляет
за клиентом тот же backend. Backend освобождается только после idle-статуса
`I`, обычно после `COMMIT` или `ROLLBACK`.

Fastpath-вызовы в autocommit освобождают backend сразу после `ReadyForQuery` со
статусом idle.

Это соответствует поведению PgBouncer в transaction pooling для трафика
`FunctionCall`.

## Размер пула

Каждый активный вызов функции large object занимает один backend до
`ReadyForQuery`. Считайте размер пула по числу одновременных чтений и записей
large objects, а не только по обычному темпу SQL-запросов.

После включения такого трафика следите за:

- `SHOW POOLS`: активные клиенты, активные серверы и ожидающие клиенты.
- Ошибками `query_wait_timeout`.
- Перцентилями задержек для пулов с large object трафиком.

Если всплески вызовов large object подводят клиентов к `query_wait_timeout`,
увеличьте пул для нужной пары user/database или уменьшите параллелизм large
object операций в приложении.

## Большие чтения

pg_doorman передаёт большие `DataRow`, `CopyData` и `FunctionCallResponse`
потоково, если они превышают `general.message_size_to_be_stream`. Большой
fastpath-ответ `lo_read` уходит клиенту без предварительного буферизования
всего ответа в памяти pg_doorman.

Потоковая передача ограничивает расход heap в pg_doorman, но не делает большие
одиночные чтения бесплатными. Большой `lo_read` всё равно удерживает backend и
сокетные буферы, пока PostgreSQL отправляет ответ. Лимиты сообщений протокола
PostgreSQL тоже остаются. Читайте large objects порциями на стороне приложения.

## Таймауты

`server_lifetime` применяется к idle backend в пуле. Он не прерывает backend,
который выполняет чтение или запись large object.

Дескрипторы large object зависят от состояния PostgreSQL-транзакции. Если
приложение оставляет large object транзакцию idle между fastpath-вызовами,
PostgreSQL `idle_in_transaction_session_timeout` может закрыть backend.
pg_doorman вернёт клиенту ошибку соединения. Держите large object транзакции
короткими или настройте PostgreSQL-таймауты для сессий, которые работают с
large objects.
