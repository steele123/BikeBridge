use crate::{ControllerTransport, protocol::Inputs};
use bikebridge_core::{
    BridgeError, DeviceCapability, DeviceInfo, ErrorCode, Event, EventBus, Result, timestamp_ms,
};
use futures_util::{StreamExt, stream::BoxStream};
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use tokio::{
    sync::{RwLock, mpsc, oneshot},
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Independent per-controller workers. Connecting a viewer never claims trainer control.
#[derive(Clone)]
pub struct Controllers {
    entries: Arc<RwLock<HashMap<String, Entry>>>,
    bus: EventBus,
    stop: CancellationToken,
    tasks: TaskTracker,
}
struct Entry {
    info: Arc<RwLock<DeviceInfo>>,
    transport: Arc<dyn ControllerTransport>,
    requests: Option<mpsc::Sender<Request>>,
}
struct Request {
    connected: bool,
    reply: oneshot::Sender<Result<DeviceInfo>>,
}
impl Controllers {
    /// Create a controller registry without touching Bluetooth.
    pub fn new(bus: EventBus) -> Self {
        Self {
            entries: Arc::default(),
            bus,
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }
    /// Retain a transport under an opaque device ID; preserve active session state on rediscovery.
    pub async fn register(&self, info: &DeviceInfo, transport: Arc<dyn ControllerTransport>) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get(&info.id) {
            let mut current = entry.info.write().await;
            current.name.clone_from(&info.name);
            current.signal_strength = info.signal_strength;
        } else {
            entries.insert(
                info.id.clone(),
                Entry {
                    info: Arc::new(RwLock::new(info.clone())),
                    transport,
                    requests: None,
                },
            );
        }
    }
    /// Whether this registry owns an identity.
    pub async fn contains(&self, id: &str) -> bool {
        self.entries.read().await.contains_key(id)
    }
    /// Overlay verified connection state onto a discovery snapshot.
    pub async fn overlay(&self, info: &mut DeviceInfo) {
        if let Some(entry) = self.entries.read().await.get(&info.id) {
            let current = entry.info.read().await;
            info.connected = current.connected;
            info.capabilities.clone_from(&current.capabilities);
        }
    }
    /// Publish discovery while holding the same metadata lock as connection transitions.
    pub async fn publish_discovery(&self, mut event: Event) {
        if let Event::DeviceDiscovered { data } | Event::DeviceUpdated { data } = &mut event
            && let Some(entry) = self.entries.read().await.get(&data.id)
        {
            let current = entry.info.read().await;
            data.connected = current.connected;
            data.capabilities.clone_from(&current.capabilities);
            self.bus.publish(event);
        } else {
            self.bus.publish(event);
        }
    }
    /// Explicit process-wide connection. Disconnect cancels automatic retries and releases held inputs.
    pub async fn set_connection(&self, id: &str, connected: bool) -> Result<DeviceInfo> {
        let (reply, response) = oneshot::channel();
        {
            let mut entries = self.entries.write().await;
            if self.stop.is_cancelled() {
                return Err(disconnected());
            }
            let entry = entries.get_mut(id).ok_or_else(|| {
                BridgeError::new(ErrorCode::DeviceNotFound, "Controller not found.")
            })?;
            if entry.requests.is_none() {
                if !connected {
                    return Ok(entry.info.read().await.clone());
                }
                let (sender, receiver) = mpsc::channel(4);
                self.tasks.spawn(
                    Worker {
                        info: entry.info.clone(),
                        transport: entry.transport.clone(),
                        bus: self.bus.clone(),
                        stop: self.stop.clone(),
                        inputs: Inputs::default(),
                        active: None,
                        needs_cleanup: false,
                    }
                    .run(receiver),
                );
                entry.requests = Some(sender);
            }
            entry
                .requests
                .as_ref()
                .ok_or_else(disconnected)?
                .try_send(Request { connected, reply })
                .map_err(|_| {
                    BridgeError::new(
                        ErrorCode::Busy,
                        "Controller connection queue is unavailable.",
                    )
                })?;
        }
        bounded(25, async { response.await.map_err(|_| disconnected())? }).await
    }
    /// Cancel and join all workers, with bounded transport teardown.
    pub async fn shutdown(&self) {
        // Serialize against lazy worker creation.
        let entries = self.entries.write().await;
        self.stop.cancel();
        self.tasks.close();
        drop(entries);
        self.tasks.wait().await;
    }
}
struct Worker {
    info: Arc<RwLock<DeviceInfo>>,
    transport: Arc<dyn ControllerTransport>,
    bus: EventBus,
    stop: CancellationToken,
    inputs: Inputs,
    active: Option<BoxStream<'static, Vec<u8>>>,
    needs_cleanup: bool,
}
impl Worker {
    async fn mark(&mut self, connected: bool) -> DeviceInfo {
        let mut info = self.info.write().await;
        let changed = info.connected != connected;
        info.connected = connected;
        if connected {
            info.capabilities = vec![DeviceCapability::ControllerInput];
        }
        if changed {
            self.bus.publish(if connected {
                Event::DeviceConnected { data: info.clone() }
            } else {
                Event::DeviceDisconnected { data: info.clone() }
            });
        }
        info.clone()
    }
    async fn emit(&mut self, data: Vec<bikebridge_core::InputData>) {
        let id = self.info.read().await.id.clone();
        for data in data {
            self.bus.publish(Event::Input {
                device_id: id.clone(),
                data,
                timestamp_ms: timestamp_ms(),
            });
        }
    }
    async fn close(&mut self) -> Result<DeviceInfo> {
        self.active = None;
        let releases = self.inputs.release_all();
        self.emit(releases).await;
        let result = if self.needs_cleanup {
            bounded(3, self.transport.disconnect()).await
        } else {
            Ok(())
        };
        self.needs_cleanup = result.is_err();
        let info = self.mark(false).await;
        if let Err(error) = &result {
            self.bus.publish(Event::Error {
                data: error.clone(),
            });
        }
        result.map(|()| info)
    }
    async fn open(&mut self) -> Result<DeviceInfo> {
        if self.needs_cleanup {
            bounded(3, self.transport.disconnect()).await?;
        }
        self.needs_cleanup = true;
        self.active = Some(bounded(10, self.transport.open()).await?);
        Ok(self.mark(true).await)
    }
    async fn retry(&mut self, attempt: usize) -> Option<(usize, Instant)> {
        let delay = *[1, 2, 5, 10, 30].get(attempt)?;
        self.bus.publish(Event::DeviceReconnecting {
            device_id: self.info.read().await.id.clone(),
            attempt: attempt as u8 + 1,
            delay_seconds: delay,
        });
        Some((attempt, Instant::now() + Duration::from_secs(delay)))
    }
    async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        let mut retry: Option<(usize, Instant)> = None;
        let mut link = tokio::time::interval(Duration::from_secs(1));
        link.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut rate_window = Instant::now();
        let mut packets = 0u32;
        loop {
            let stop = self.stop.clone();
            tokio::select! {
                biased;
                _=stop.cancelled()=>break,
                request=requests.recv()=>{
                    let Some(mut request)=request else {break};
                    if request.reply.is_closed() {continue;}
                    retry=None;
                    let new_connection=request.connected && self.active.is_none();
                    let result=if new_connection {
                        tokio::select! {
                            _=stop.cancelled()=>Err(disconnected()),
                            _=request.reply.closed()=>Err(disconnected()),
                            result=self.open()=>result,
                        }
                    } else if request.connected {Ok(self.info.read().await.clone())} else {self.close().await};
                    let failed=result.is_err();
                    if let Err(error)=&result {self.bus.publish(Event::Error {data:error.clone()});}
                    if (request.reply.send(result).is_err() || failed) && new_connection {let _=self.close().await;}
                    rate_window=Instant::now();packets=0;
                }
                _=async {match retry {Some((_,at))=>tokio::time::sleep_until(at).await,None=>std::future::pending().await}}=>{
                    let attempt=retry.take().map(|(n,_)|n).unwrap_or(0);
                    let result=tokio::select! {_=stop.cancelled()=>Err(disconnected()),result=self.open()=>result};
                    if let Err(error)=result {
                        self.bus.publish(Event::Error {data:error});
                        let _=self.close().await;
                        if !stop.is_cancelled() {retry=self.retry(attempt+1).await;}
                    }
                    rate_window=Instant::now();packets=0;
                }
                packet=async {match &mut self.active {Some(stream)=>stream.next().await,None=>std::future::pending().await}}=>{
                    match packet {
                        Some(bytes)=>{
                            if rate_window.elapsed()>=Duration::from_secs(1) {rate_window=Instant::now();packets=0;}
                            packets+=1;
                            let result=if packets>1000 {Err(BridgeError::new(ErrorCode::InvalidDeviceData,"Controller exceeded the input packet rate limit."))} else {self.transport.decode(&bytes).and_then(|data| self.inputs.update(data))};
                            match result {
                                Ok(data)=>self.emit(data).await,
                                Err(error)=>{self.bus.publish(Event::Error {data:error});let _=self.close().await;retry=None;}
                            }
                        }
                        None=>{self.bus.publish(Event::Error {data:disconnected()});let _=self.close().await;retry=self.retry(0).await;}
                    }
                }
                _=link.tick(),if self.active.is_some()=>{
                    let result=tokio::select! {_=stop.cancelled()=>break,result=bounded(3,self.transport.is_connected())=>result};
                    if !matches!(result,Ok(true)) {
                        self.bus.publish(Event::Error {data:result.err().unwrap_or_else(disconnected)});
                        let _=self.close().await;retry=self.retry(0).await;
                    }
                }
            }
        }
        let _ = self.close().await;
    }
}
fn disconnected() -> BridgeError {
    BridgeError::new(
        ErrorCode::DeviceDisconnected,
        "Controller connection is unavailable.",
    )
}
async fn bounded<T>(seconds: u64, future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(seconds), future)
        .await
        .map_err(|_| {
            BridgeError::new(
                ErrorCode::Timeout,
                "Controller transport operation timed out.",
            )
        })?
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
