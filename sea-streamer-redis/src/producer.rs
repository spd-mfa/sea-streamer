use flume::{bounded, r#async::RecvFut, unbounded, Sender};
use redis::{aio::ConnectionLike, cmd as command, ErrorKind};
use std::{fmt::Debug, future::Future, sync::Arc, time::Duration};

use crate::{
    map_err, parse_message_id, string_from_redis_value, RedisCluster, RedisErr, RedisResult, MSG,
    ZERO,
};
use sea_streamer_runtime::{sleep, spawn_task};
use sea_streamer_types::{
    export::{async_trait, futures::FutureExt},
    Buffer, MessageHeader, Producer, ProducerOptions, Receipt, ShardId, StreamErr, StreamKey,
    Timestamp,
};

const SEA_STREAMER_INTERNAL: &str = "SEA_STREAMER_INTERNAL";
const MAX_RETRY: usize = 100;

#[derive(Debug, Clone)]
pub struct RedisProducer {
    stream: Option<StreamKey>,
    sender: Sender<SendRequest>,
}

#[derive(Default, Clone)]
pub struct RedisProducerOptions {
    sharder: Option<Arc<dyn SharderConfig>>,
}

impl Debug for RedisProducerOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisProducerOptions")
            .field("sharder", &self.sharder.as_ref())
            .finish()
    }
}

struct SendRequest {
    stream_key: StreamKey,
    bytes: Vec<u8>,
    receipt: Sender<RedisResult<Receipt>>,
}

pub struct SendFuture {
    fut: RecvFut<'static, RedisResult<Receipt>>,
}

impl Debug for SendFuture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendFuture").finish()
    }
}

/// Struct to bootstrap sharders
pub trait SharderConfig: Debug + Send + Sync {
    /// Each producer will create its own sharder.
    /// They should not have any shared state for the sake of concurrency.
    fn init(&self) -> Box<dyn Sharder>;
}

/// Custom sharding strategy
pub trait Sharder: Send {
    /// Return the determined shard id for the given message.
    /// This should be a *real quick* computation, otherwise this can become the bottleneck of streaming.
    /// Mutex, atomic or anything that can create contention will be disastrous.
    ///
    /// It will then be sent to the stream with key `STREAM_KEY:SHARD`.
    /// The Redis Cluster will allocate this shard to a particular node as the cluster scales.
    /// Different shards may or may not end up in the same slot, and thus may or may not end up in the same node.
    fn shard(&mut self, stream_key: &StreamKey, bytes: &[u8]) -> u64;
}

#[derive(Debug)]
/// Shard streams pseudo-randomly but fairly. Basically a `rand() / num_shards`.
pub struct PseudoRandomSharder {
    num_shards: u64,
}

#[async_trait]
impl Producer for RedisProducer {
    type Error = RedisErr;
    type SendFuture = SendFuture;

    fn send_to<S: Buffer>(&self, stream: &StreamKey, payload: S) -> RedisResult<Self::SendFuture> {
        // one shot channel
        let (sender, receiver) = bounded(1);
        // unbounded, so never blocks
        self.sender
            .send(SendRequest {
                stream_key: stream.to_owned(),
                bytes: payload.into_bytes(),
                receipt: sender,
            })
            .map_err(|_| StreamErr::Backend(RedisErr::ProducerDied))?;

        Ok(SendFuture {
            fut: receiver.into_recv_async(),
        })
    }

    #[inline]
    async fn flush(self) -> RedisResult<()> {
        self.flush_once().await
    }

    fn anchor(&mut self, stream: StreamKey) -> RedisResult<()> {
        if self.stream.is_none() {
            self.stream = Some(stream);
            Ok(())
        } else {
            Err(StreamErr::AlreadyAnchored)
        }
    }

    fn anchored(&self) -> RedisResult<&StreamKey> {
        if let Some(stream) = &self.stream {
            Ok(stream)
        } else {
            Err(StreamErr::NotAnchored)
        }
    }
}

impl RedisProducer {
    /// Like [`ProducerTrait::flush`], but does not destroy one self.
    pub async fn flush_once(&self) -> RedisResult<()> {
        // The trick here is to send a signal message and wait for the receipt.
        // By the time it returns a receipt, everything before should have already been sent.
        let null = [];
        self.send_to(&StreamKey::new(SEA_STREAMER_INTERNAL)?, null.as_slice())?
            .await?;
        Ok(())
    }
}

impl ProducerOptions for RedisProducerOptions {}

impl Future for SendFuture {
    type Output = RedisResult<MessageHeader>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.fut.poll_unpin(cx) {
            std::task::Poll::Ready(res) => std::task::Poll::Ready(match res {
                Ok(res) => res,
                Err(_) => Err(StreamErr::Backend(RedisErr::ProducerDied)),
            }),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

pub(crate) async fn create_producer(
    mut cluster: RedisCluster,
    mut options: RedisProducerOptions,
) -> RedisResult<RedisProducer> {
    cluster.reconnect_all().await?; // init connections
    let (sender, receiver) = unbounded();
    let mut sharder = options.sharder.take().map(|a| a.init());

    // Redis commands are exclusive (`&mut self`), so we need a producer task
    spawn_task(async move {
        // exit if all senders have been dropped
        while let Ok(SendRequest {
            stream_key,
            bytes,
            receipt,
        }) = receiver.recv_async().await
        {
            if stream_key.name() == SEA_STREAMER_INTERNAL && bytes.is_empty() {
                // A signalling message
                receipt
                    .send_async(Ok(MessageHeader::new(
                        stream_key,
                        ZERO,
                        0,
                        Timestamp::now_utc(),
                    )))
                    .await
                    .ok();
            } else {
                let mut cmd = command("XADD");
                let redis_stream_key;
                let (redis_key, shard) = if let Some(sharder) = sharder.as_mut() {
                    let shard = sharder.shard(&stream_key, bytes.as_slice());
                    redis_stream_key = format!("{name}:{shard}", name = stream_key.name());
                    (redis_stream_key.as_str(), ShardId::new(shard))
                } else {
                    (stream_key.name(), ZERO)
                };
                cmd.arg(redis_key);
                cmd.arg("*");
                let msg = [(MSG, bytes)];
                cmd.arg(&msg);
                let (mut retried, mut asked) = (0, 0);
                let result = loop {
                    let (node, conn) = match cluster.get_connection_for(redis_key).await {
                        Ok(conn) => conn,
                        Err(StreamErr::Backend(RedisErr::TryAgain(_))) => continue, // it will sleep inside `get_connection`
                        Err(err) => {
                            log::error!("{err:?}");
                            return; // this will kill the producer
                        }
                    };
                    match conn.req_packed_command(&cmd).await {
                        Ok(id) => {
                            break match string_from_redis_value(id) {
                                Ok(id) => match parse_message_id(&id) {
                                    Ok((timestamp, sequence)) => Ok(MessageHeader::new(
                                        stream_key, shard, sequence, timestamp,
                                    )),
                                    Err(err) => Err(err),
                                },
                                Err(err) => Err(err),
                            }
                        }
                        Err(err) => {
                            retried += 1;
                            if retried == MAX_RETRY {
                                panic!(
                                    "The cluster might have a problem. Already retried {retried} times."
                                );
                            }
                            let kind = err.kind();
                            if kind == ErrorKind::Moved {
                                cluster.moved(
                                    redis_key,
                                    match err.redirect_node() {
                                        Some((to, _slot)) => {
                                            // `to` must be in form of `host:port` without protocol
                                            format!("{}://{}", cluster.protocol().unwrap(), to)
                                                .parse()
                                                .expect("Failed to parse URL: {to}")
                                        }
                                        None => panic!("Key is moved, but to where? {err:?}"),
                                    },
                                );
                            } else if matches!(
                                kind,
                                ErrorKind::Ask
                                    | ErrorKind::TryAgain
                                    | ErrorKind::ClusterDown
                                    | ErrorKind::MasterDown
                            ) {
                                // If it's an ASK, we wait until it finished moving.
                                // What benefits, in stream producing terms, does ASK give?
                                // This is an exponential backoff, in seq of [1, 2, 4, 8, 16, 32, 64].
                                sleep(Duration::from_secs(1 << std::cmp::min(6, asked))).await;
                                asked += 1;
                            } else if kind == ErrorKind::IoError {
                                let node = node.to_owned();
                                cluster.reconnect(&node).ok();
                            } else {
                                // unrecoverable
                                break Err(map_err(err));
                            }
                        }
                    }
                };
                receipt.send_async(result).await.ok();
            }
        }
    });

    Ok(RedisProducer {
        stream: None,
        sender,
    })
}

impl PseudoRandomSharder {
    pub fn new_config(num_shards: u64) -> Arc<dyn SharderConfig> {
        Arc::new(Self { num_shards })
    }
}

impl SharderConfig for PseudoRandomSharder {
    fn init(&self) -> Box<dyn Sharder> {
        let new = Self {
            num_shards: self.num_shards,
        };
        Box::new(new)
    }
}

impl Sharder for PseudoRandomSharder {
    fn shard(&mut self, _: &StreamKey, _: &[u8]) -> u64 {
        Timestamp::now_utc().millisecond() as u64 % self.num_shards
    }
}