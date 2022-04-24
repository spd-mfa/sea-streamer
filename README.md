# SeaStreamer

The universal stream processing toolkit

# Background

https://www.confluent.io/blog/turning-the-database-inside-out-with-apache-samza/ is an influential article in the data engineering space, and lead to the founding of [Kafka](https://kafka.apache.org/). Since then, competing implementations like [Redpanda](https://redpanda.com/) and [Redis Streams](https://redis.io/docs/manual/data-types/streams/) emerged, spurring a real-time data processing ecosystem, with projects like [ksqlDB](https://ksqldb.io/) and [Materialize](https://materialize.com/) built on top / around.

# Statement of Intent

Similar to what SeaORM promises, we want to make Rust the best language for stream processing, and construct a data engineering platform (free of JVM drawbacks) featuring Rust's low latency, low memory footprint, suitable for long running (no GC pause and memory leaks).

Similar to SeaORM, we will provide a Stream library which is high-level, abstract and backend agnostic.

Similar to SeaQuery, we will provide a generic driver library implementing the `Streamer` trait to support different backends.

Similar to SeaSchema, we will also provide a programmable API for administration of brokers and clusters.

(But we may keep everything under the same repo for now)

Finally, we want to provide an array of command line tools for manual data maintenance, usable in the unix shell!

Let me illustrate this extravagant concept:

```sh
# we setup a `flux` which maps a unix file to a stream producer
$ sea streamer flux ./producer --broker localhost:9092 --topic news &
# we want to stream the content of the Newline Delimited JSON file
$ cat news.ndjson > producer
# kill the process and then the ./producer file will be unlinked
$ kill %1
```

Likewise, we can tap in a stream:

```sh
$ sea streamer tap ./consumer --broker localhost:9092 --topic news &
$ cat consumer > news.ndjson
$ kill %1
```

Sweet, isn't it?

# Concepts

### Stream

A stream (it's actually a topic in Kafka) consist of a series of messages with a timestamp, sequence number (known as offset in Kafka) and shard id (known as partition number in Kafka). A message is uniquely identified by the (shard id, sequence number) pair.

The stream can be sought to a particular timestamp or sequence number.

Stream data has a retention period (how long before data will be deleted).

### Consumer

A stream consumer subscribes to one or more streams and receive messages from one or more node in the cluster.

According to the use case, there can be several consumption preferences:

1. latest: we only care about latest messages and would be okay to miss old data
2. repeat: we should process all messages, but wouldn't mind processing the same message more than once
3. exactly once: each message must be processed and be processed exactly once

### Producer

A stream producer send messages to a broker, and the broker would forward to a node in the cluster. There can be logic in how to shard a stream, but usually it's pseudo-random.

According to the use case, there can be several durability requirements:

1. fire and forget (at most once): basically no guarantee that a message will be persisted
2. at least once: we would try to deliver the message only once, but might end up more than once upon network failure (basically we want to retry until we receive an ack)
3. exactly once: basically the broker has to have a buffer to be able to remove duplicate messages, which means we cannot guarantee uniqueness across the entire stream, only a specific time window

### Processor

A stream processor is a consumer and producer at the same time. In a nutshell, it consumes stream, transforms them (perform some computation) and produces another stream.

We aim to make it extremely easy and flexible to create stream processors:

Eventually we will have a stream processing engine as an Enterprise component that schedules (start/stop), manages (add/drop) stream processors.

Using [pyo3](https://github.com/PyO3/pyo3), stream processors can be written in Python. This will be our 1st support target.

### Controller

Stream processors have states. A pure stream processor has its state determined deterministically by the stream input. This is a crucial property that a processor should uphold, and we expect to be able to reproduce a processor's output given the exact same input.

A controller is responsible for feeding inputs to a stream processor and record the log e.g. "Feed steam topic "XX" shard "YY" offset 123 to 456 to processor "ZZZ"

And so, if something goes wrong, we will be able to rewind and replay the processing and inspect the internal state to debug the exact problem.

As a side effect, processors cannot have access to `random` and `time`, among other causes of non-determinism.

The engine has to wrap and log these in reproducible ways.

### Subscribers

Finally, we also want to make it extremely simple to create realtime client-side applications.

For example, in a trading app, the prices of assets keep updating in real-time!

These apps stream real-time data from server through web sockets, and so here is another Enterprise component: a web socket server that hosts many clients, and manage their stream subscriptions. i.e. one client can subscribe to multiple stream and sub/unsub dynamically as they wish.

As such, the web socket server channels internal streams (Kafka / Redis) to the external world (websocket, or webhook if the stream is sparse).