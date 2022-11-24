use crate::{Message, Result, SequenceNo, ShardId};
use async_trait::async_trait;
use futures::Stream;
use time::PrimitiveDateTime as DateTime;

#[derive(Debug)]
pub enum ConsumerMode {
    /// This is the 'vanilla' stream consumer. It does not commit, and only consumes messages from now on
    RealTime,
    /// When the process restarts, it will resume the stream from the previous committed sequence.
    /// It will use a consumer id unique to this host: on a physical machine, it will use the mac address.
    /// Inside a docker container, it will use the container id.
    Resumable,
    /// You should assign a consumer group manually. The load-balancing mechanism is implementation-specific.
    LoadBalanced,
}

#[derive(Debug)]
pub struct ConsumerGroup {
    name: String,
}

pub trait ConsumerOptions: Clone + Send {
    fn new(mode: ConsumerMode) -> Self;

    /// Get currently set consumer group; may return [`StreamErr::ConsumerGroupNotSet`].
    fn consumer_group(&self) -> Result<ConsumerGroup>;

    /// Set consumer group for this consumer. Note the semantic is implementation-specific.
    fn set_consumer_group(&mut self, group_id: ConsumerGroup) -> Result<()>;
}

#[async_trait]
pub trait Consumer: Sized + Send + Sync {
    type Stream: Stream<Item = Message>;

    /// seek to an arbitrary point in time; start consuming the closest message
    fn seek(&self, to: DateTime);
    /// rewind the stream to a particular sequence number
    fn rewind(&self, seq: SequenceNo);
    /// assign this consumer to a particular shard
    fn assign(&self, shard: ShardId);
    /// poll and receive one message: it waits until there are new messages
    async fn next(&self) -> Message;
    /// returns an async stream
    fn stream(self) -> Self::Stream;
}

impl ConsumerGroup {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}
