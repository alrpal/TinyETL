//! Streaming query results for database sources.
//!
//! A producer (an async task or a thread) runs one query and pushes converted rows through a
//! bounded channel in chunks; the source pulls batches of any size from the other end. One
//! query replaces `LIMIT … OFFSET` paging, which without an `ORDER BY` can return overlapping
//! or missing rows between pages, and rescans the skipped rows on every page.

use std::collections::VecDeque;
use std::future::Future;
use tokio::sync::mpsc;

use crate::{schema::Row, Result, TinyEtlError};

/// Rows per chunk sent through the channel.
const CHUNK_ROWS: usize = 1024;

/// Chunks buffered ahead of the reader (about 8k rows).
const CHUNKS_AHEAD: usize = 8;

/// Errors cross as text: `TinyEtlError` isn't `Send`.
type Chunk = std::result::Result<Vec<Row>, String>;

/// The producer's end: collects rows into chunks and sends them.
pub(crate) struct RowSink {
    sender: mpsc::Sender<Chunk>,
    buffer: Vec<Row>,
}

impl RowSink {
    /// Queue a row. Returns false once the reader is gone; the producer should stop.
    pub async fn push(&mut self, row: Row) -> bool {
        self.buffer.push(row);
        if self.buffer.len() >= CHUNK_ROWS {
            return self.flush().await;
        }
        true
    }

    /// [`RowSink::push`] for producers running on a thread.
    pub fn blocking_push(&mut self, row: Row) -> bool {
        self.buffer.push(row);
        if self.buffer.len() >= CHUNK_ROWS {
            return self.blocking_flush();
        }
        true
    }

    async fn flush(&mut self) -> bool {
        if self.buffer.is_empty() {
            return true;
        }
        let chunk = std::mem::replace(&mut self.buffer, Vec::with_capacity(CHUNK_ROWS));
        self.sender.send(Ok(chunk)).await.is_ok()
    }

    fn blocking_flush(&mut self) -> bool {
        if self.buffer.is_empty() {
            return true;
        }
        let chunk = std::mem::replace(&mut self.buffer, Vec::with_capacity(CHUNK_ROWS));
        self.sender.blocking_send(Ok(chunk)).is_ok()
    }

    /// Send the remaining rows; the stream ends when the sink is dropped.
    pub async fn finish(mut self) {
        self.flush().await;
    }

    pub fn blocking_finish(mut self) {
        self.blocking_flush();
    }

    /// End the stream with an error (after the rows already queued).
    pub async fn fail(mut self, message: String) {
        if self.flush().await {
            let _ = self.sender.send(Err(message)).await;
        }
    }

    pub fn blocking_fail(mut self, message: String) {
        if self.blocking_flush() {
            let _ = self.sender.blocking_send(Err(message));
        }
    }
}

/// The reader's end.
pub(crate) struct RowStream {
    receiver: mpsc::Receiver<Chunk>,
    pending: VecDeque<Row>,
    /// An error received while looking ahead, returned by the next read
    error: Option<String>,
    done: bool,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RowStream {
    fn channel() -> (RowSink, mpsc::Receiver<Chunk>) {
        let (sender, receiver) = mpsc::channel(CHUNKS_AHEAD);
        (
            RowSink {
                sender,
                buffer: Vec::with_capacity(CHUNK_ROWS),
            },
            receiver,
        )
    }

    fn from_receiver(
        receiver: mpsc::Receiver<Chunk>,
        task: Option<tokio::task::JoinHandle<()>>,
    ) -> Self {
        Self {
            receiver,
            pending: VecDeque::new(),
            error: None,
            done: false,
            task,
        }
    }

    /// Run an async producer on the Tokio runtime.
    pub fn spawn<F, Fut>(produce: F) -> Self
    where
        F: FnOnce(RowSink) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (sink, receiver) = Self::channel();
        let task = tokio::spawn(produce(sink));
        Self::from_receiver(receiver, Some(task))
    }

    /// Run a blocking producer (e.g. a synchronous database driver) on its own thread.
    pub fn spawn_blocking<F>(produce: F) -> Result<Self>
    where
        F: FnOnce(RowSink) + Send + 'static,
    {
        let (sink, receiver) = Self::channel();
        std::thread::Builder::new()
            .name("tinyetl-db-reader".to_string())
            .spawn(move || produce(sink))
            .map_err(TinyEtlError::Io)?;
        Ok(Self::from_receiver(receiver, None))
    }

    async fn receive(&mut self) {
        match self.receiver.recv().await {
            Some(Ok(rows)) => self.pending.extend(rows),
            Some(Err(message)) => {
                self.error = Some(message);
                self.done = true;
            }
            None => self.done = true,
        }
    }

    /// Up to `batch_size` rows; fewer only at the end of the result.
    pub async fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        while self.pending.len() < batch_size && !self.done {
            self.receive().await;
        }
        if self.pending.is_empty() {
            if let Some(message) = self.error.take() {
                return Err(TinyEtlError::DataTransfer(message));
            }
        }
        let n = batch_size.min(self.pending.len());
        let rows: Vec<Row> = self.pending.drain(..n).collect();
        // Look ahead so has_more() is exact
        if self.pending.is_empty() && !self.done {
            self.receive().await;
        }
        Ok(rows)
    }

    pub fn has_more(&self) -> bool {
        !self.pending.is_empty() || !self.done || self.error.is_some()
    }
}

impl Drop for RowStream {
    fn drop(&mut self) {
        // Stop a producer that's still running; a thread producer stops at its next push
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Value;

    fn row(i: i64) -> Row {
        Row::from([("i".to_string(), Value::Integer(i))])
    }

    #[tokio::test]
    async fn test_batches_of_any_size() {
        let mut stream = RowStream::spawn(|mut sink| async move {
            for i in 0..5000 {
                if !sink.push(row(i)).await {
                    return;
                }
            }
            sink.finish().await;
        });
        let mut seen = Vec::new();
        while stream.has_more() {
            let batch = stream.next_batch(700).await.unwrap();
            assert!(batch.len() <= 700);
            seen.extend(batch.into_iter().map(|r| r["i"].clone()));
        }
        assert_eq!(seen.len(), 5000);
        assert_eq!(seen[4999], Value::Integer(4999));
        assert!(stream.next_batch(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_error_after_rows() {
        let mut stream = RowStream::spawn_blocking(|mut sink| {
            for i in 0..3 {
                sink.blocking_push(row(i));
            }
            sink.blocking_fail("connection lost".to_string());
        })
        .unwrap();
        assert_eq!(stream.next_batch(10).await.unwrap().len(), 3);
        assert!(stream.has_more());
        let err = stream.next_batch(10).await.unwrap_err();
        assert!(err.to_string().contains("connection lost"));
    }

    #[tokio::test]
    async fn test_empty_result() {
        let mut stream = RowStream::spawn(|sink| async move { sink.finish().await });
        assert!(stream.next_batch(10).await.unwrap().is_empty());
        assert!(!stream.has_more());
    }

    #[tokio::test]
    async fn test_dropping_the_reader_stops_the_producer() {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let stream = RowStream::spawn_blocking(move |mut sink| {
            let mut i = 0;
            while sink.blocking_push(row(i)) {
                i += 1;
            }
            let _ = done_tx.send(i);
        })
        .unwrap();
        drop(stream);
        let pushed = done_rx.await.unwrap();
        assert!(pushed < 100_000, "producer kept going: {}", pushed);
    }
}
