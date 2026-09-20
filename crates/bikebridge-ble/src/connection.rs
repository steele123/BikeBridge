use crate::{
    ftms::decode_features,
    session::{ActiveSession, SessionEvent},
    transport::FtmsTransport,
};
use bikebridge_core::{
    BridgeError, DeviceInfo, ErrorCode, Event, EventBus, Result, SafetyLimits, TrainerCommand,
};
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;

#[derive(Clone)]
pub(crate) struct Connections {
    entries: Arc<RwLock<HashMap<String, Entry>>>,
    bus: EventBus,
    stop: CancellationToken,
    tasks: TaskTracker,
    limits: SafetyLimits,
    leases: Arc<RwLock<HashMap<usize, CancellationToken>>>,
}
struct Entry {
    info: Arc<RwLock<DeviceInfo>>,
    transport: Arc<dyn FtmsTransport>,
    commands: Option<mpsc::Sender<Request>>,
}
enum Request {
    Connection {
        client: usize,
        connected: bool,
        reply: oneshot::Sender<Result<DeviceInfo>>,
    },
    Control {
        client: usize,
        lease: CancellationToken,
        command: TrainerCommand,
        reply: oneshot::Sender<Result<TrainerCommand>>,
    },
}

impl Connections {
    #[cfg(test)]
    pub fn new(bus: EventBus) -> Self {
        Self::with_limits(bus, SafetyLimits::default())
    }
    pub fn with_limits(bus: EventBus, limits: SafetyLimits) -> Self {
        Self {
            limits,
            leases: Arc::default(),
            entries: Arc::default(),
            bus,
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }
    pub async fn register(&self, info: &DeviceInfo, transport: Arc<dyn FtmsTransport>) {
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
                    commands: None,
                },
            );
        }
    }
    pub async fn overlay(&self, info: &mut DeviceInfo) {
        if let Some(entry) = self.entries.read().await.get(&info.id) {
            let current = entry.info.read().await;
            info.connected = current.connected;
            info.capabilities.clone_from(&current.capabilities);
        }
    }
    pub async fn publish_discovery(&self, mut event: Event) {
        if let Event::DeviceDiscovered { data } | Event::DeviceUpdated { data } = &mut event
            && let Some(entry) = self.entries.read().await.get(&data.id)
        {
            // Keep publication under the same metadata lock as connection events:
            // a stale advertisement must never follow a newer connection transition.
            let current = entry.info.read().await;
            data.connected = current.connected;
            data.capabilities.clone_from(&current.capabilities);
            self.bus.publish(event);
        } else {
            self.bus.publish(event);
        }
    }
    #[cfg(test)]
    pub async fn set_connection(&self, id: &str, connected: bool) -> Result<DeviceInfo> {
        self.set_connection_for(0, id, connected).await
    }
    pub async fn set_connection_for(
        &self,
        client: usize,
        id: &str,
        connected: bool,
    ) -> Result<DeviceInfo> {
        let (reply, response) = oneshot::channel();
        {
            let mut entries = self.entries.write().await;
            if self.stop.is_cancelled() {
                return Err(stopped());
            }
            let entry = entries.get_mut(id).ok_or_else(|| {
                BridgeError::new(
                    ErrorCode::UnsupportedOperation,
                    "Only discovered FTMS indoor bikes can connect.",
                )
            })?;
            if entry.commands.is_none() {
                if !connected {
                    return Ok(entry.info.read().await.clone());
                }
                let (commands, requests) = mpsc::channel(4);
                self.tasks.spawn(run(
                    WorkerContext {
                        transport: entry.transport.clone(),
                        info: entry.info.clone(),
                        bus: self.bus.clone(),
                        stop: self.stop.clone(),
                        limits: self.limits,
                    },
                    requests,
                ));
                entry.commands = Some(commands);
            }
            entry
                .commands
                .as_ref()
                .ok_or_else(stopped)?
                .try_send(Request::Connection {
                    client,
                    connected,
                    reply,
                })
                .map_err(|_| {
                    BridgeError::new(
                        ErrorCode::Busy,
                        "Device command queue is unavailable. Retry later.",
                    )
                })?;
        }
        tokio::time::timeout(Duration::from_secs(25), response)
            .await
            .map_err(|_| BridgeError::new(ErrorCode::Timeout, "Device request timed out."))?
            .map_err(|_| stopped())?
    }
    pub async fn execute(
        &self,
        client: usize,
        id: &str,
        command: TrainerCommand,
    ) -> Result<TrainerCommand> {
        self.limits.clamp(command)?;
        let (reply, response) = oneshot::channel();
        {
            let entries = self.entries.read().await;
            if self.stop.is_cancelled() {
                return Err(stopped());
            }
            let entry = entries
                .get(id)
                .ok_or_else(|| crate::control::unsupported("Device is not an FTMS trainer."))?;
            let commands = entry
                .commands
                .as_ref()
                .ok_or_else(crate::session::disconnected)?;
            let lease = self.leases.write().await.entry(client).or_default().clone();
            commands
                .try_send(Request::Control {
                    client,
                    lease,
                    command,
                    reply,
                })
                .map_err(|_| {
                    BridgeError::new(ErrorCode::Busy, "Device command queue is unavailable.")
                })?;
        }
        tokio::time::timeout(Duration::from_secs(20), response)
            .await
            .map_err(|_| {
                BridgeError::new(ErrorCode::Timeout, "Trainer command request timed out.")
            })?
            .map_err(|_| stopped())?
    }
    pub async fn release_client(&self, client: usize) {
        if let Some(lease) = self.leases.write().await.remove(&client) {
            lease.cancel();
        }
    }
    pub async fn shutdown(&self) {
        // Serialize cancellation with session creation before closing the tracker.
        let entries = self.entries.write().await;
        self.stop.cancel();
        self.tasks.close();
        drop(entries);
        self.tasks.wait().await;
    }
}
fn stopped() -> BridgeError {
    BridgeError::new(
        ErrorCode::DeviceDisconnected,
        "BLE sessions are shutting down.",
    )
}
async fn bounded<T>(duration: u64, future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(duration), future)
        .await
        .map_err(|_| BridgeError::new(ErrorCode::Timeout, "BLE session operation timed out."))?
}
async fn mark(info: &RwLock<DeviceInfo>, connected: bool, bus: &EventBus) -> DeviceInfo {
    let mut info = info.write().await;
    let changed = info.connected != connected;
    info.connected = connected;
    let data = info.clone();
    if changed {
        tracing::info!(device_id = %data.id, connected, "BLE connection changed");
        bus.publish(if connected {
            Event::DeviceConnected { data: data.clone() }
        } else {
            Event::DeviceDisconnected { data: data.clone() }
        });
    }
    data
}
async fn cleanup(
    transport: &dyn FtmsTransport,
    info: &RwLock<DeviceInfo>,
    bus: &EventBus,
) -> Result<DeviceInfo> {
    let result = bounded(3, transport.disconnect()).await;
    // connected means usable telemetry session; clear it even when OS teardown fails.
    let data = mark(info, false, bus).await;
    if let Err(error) = &result {
        bus.publish(Event::Error {
            data: error.clone(),
        });
    }
    result.map(|()| data)
}
struct WorkerContext {
    transport: Arc<dyn FtmsTransport>,
    info: Arc<RwLock<DeviceInfo>>,
    bus: EventBus,
    stop: CancellationToken,
    limits: SafetyLimits,
}
impl WorkerContext {
    async fn open(&self) -> Result<ActiveSession> {
        let io = bounded(10, self.transport.open()).await?;
        let mut capabilities = decode_features(&io.features)?;
        if let Some(control) = &io.control {
            capabilities.extend(control.profile.capabilities(self.limits));
        }
        self.info.write().await.capabilities = capabilities;
        let id = self.info.read().await.id.clone();
        Ok(ActiveSession::new(
            io,
            self.transport.clone(),
            self.bus.clone(),
            id,
            self.limits,
        ))
    }
    async fn close(&self, active: &mut Option<ActiveSession>) -> Result<DeviceInfo> {
        if let Some(session) = active {
            session.safe_release().await;
        }
        *active = None;
        cleanup(self.transport.as_ref(), &self.info, &self.bus).await
    }
    fn retry(&self, id: &str, attempt: usize) -> Option<(usize, tokio::time::Instant)> {
        let delay = *[1, 2, 5, 10, 30].get(attempt)?;
        self.bus.publish(Event::DeviceReconnecting {
            device_id: id.into(),
            attempt: attempt as u8 + 1,
            delay_seconds: delay,
        });
        Some((
            attempt,
            tokio::time::Instant::now() + Duration::from_secs(delay),
        ))
    }
}
async fn run(context: WorkerContext, mut requests: mpsc::Receiver<Request>) {
    let id = context.info.read().await.id.clone();
    let mut active: Option<ActiveSession> = None;
    let mut needs_cleanup = false;
    let mut reconnect = None;
    let mut link = tokio::time::interval(Duration::from_secs(1));
    let mut ramp = tokio::time::interval(Duration::from_millis(250));
    link.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ramp.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if context.stop.is_cancelled() {
            break;
        }
        let owner = active.as_ref().and_then(|session| session.owner.clone());
        let connected = active.is_some();
        tokio::select! {
            biased;
            _=context.stop.cancelled()=>break,
            _=async {match &owner {Some((_,lease))=>lease.cancelled().await,None=>std::future::pending().await}}=>{
                needs_cleanup=context.close(&mut active).await.is_err();
                reconnect=None;
            }
            request=requests.recv()=> {
                let Some(request)=request else {break};
                match request {
                    Request::Connection {client,connected,mut reply}=>{
                        if reply.is_closed() {continue;}
                        if !connected && let Some(session)=&active && let Err(error)=session.require_owner(client) {
                            let _=reply.send(Err(error)); continue;
                        }
                        reconnect=None;
                        let result=if connected {
                            if active.is_some() {Ok(context.info.read().await.clone())}
                            else {
                                let opened=tokio::select! {
                                    _=context.stop.cancelled()=>Err(stopped()),
                                    _=reply.closed()=>Err(BridgeError::new(ErrorCode::DeviceDisconnected,"Connection requester left during setup.")),
                                    result=async {
                                        if needs_cleanup {bounded(3,context.transport.disconnect()).await?;}
                                        context.open().await
                                    }=>result,
                                };
                                match opened {
                                    Ok(session)=>{active=Some(session); needs_cleanup=true; Ok(mark(&context.info,true,&context.bus).await)}
                                    Err(error)=>{
                                        needs_cleanup=context.close(&mut active).await.is_err();
                                        context.bus.publish(Event::Error {data:error.clone()}); Err(error)
                                    }
                                }
                            }
                        } else if active.is_some() || needs_cleanup {
                            let result=context.close(&mut active).await;needs_cleanup=result.is_err();result
                        } else {Ok(context.info.read().await.clone())};
                        let _=reply.send(result);
                    }
                    Request::Control {client,lease,command,mut reply}=>{
                        if reply.is_closed() || lease.is_cancelled() {continue;}
                        let Some(session)=&mut active else {let _=reply.send(Err(crate::session::disconnected()));continue;};
                        let result=tokio::select! {
                            _=reply.closed()=>{session.cancel_uncertain();Err(BridgeError::new(ErrorCode::TrainerControlFailed,"Command caller left or timed out."))},
                            result=session.execute(client,lease,command,&context.stop)=>result,
                        };
                        let owns=session.owner.as_ref().is_some_and(|(owner,_)|*owner==client);
                        let failed=result.is_err();
                        if let Err(error)=&result {context.bus.publish(Event::Error {data:error.clone()});}
                        tracing::info!(device_id=%id,?command,success=!failed,"Trainer control command completed");
                        if (reply.send(result).is_err() || failed) && owns {
                            needs_cleanup=context.close(&mut active).await.is_err();reconnect=None;
                        }
                    }
                }
            }
            _=async {match reconnect {Some((_,deadline))=>tokio::time::sleep_until(deadline).await,None=>std::future::pending().await}}=>{
                let attempt=reconnect.take().map(|(attempt,_)|attempt).unwrap_or(0);
                tracing::info!(device_id=%id,attempt=attempt+1,"Trainer reconnect attempt");
                let opened=tokio::select! {
                    _=context.stop.cancelled()=>Err(stopped()),
                    result=async {
                        if needs_cleanup {bounded(3,context.transport.disconnect()).await?;}
                        context.open().await
                    }=>result,
                };
                match opened {
                    Ok(session)=>{active=Some(session);needs_cleanup=true;mark(&context.info,true,&context.bus).await;}
                    Err(error)=>{
                        context.bus.publish(Event::Error {data:error});
                        needs_cleanup=context.close(&mut active).await.is_err();
                        reconnect=context.retry(&id,attempt+1);
                    }
                }
            }
            event=async {match &mut active {Some(session)=>session.next_event().await,None=>std::future::pending().await}}=>{
                let SessionEvent::Control(event)=event else {
                    if let SessionEvent::Telemetry(packet)=event {
                        match packet {
                            Some(bytes)=>if let Some(session)=&mut active {session.telemetry(&bytes);},
                            None=>{
                                context.bus.publish(Event::Error {data:crate::session::disconnected()});
                                needs_cleanup=context.close(&mut active).await.is_err();
                                if context.limits.auto_reconnect {reconnect=context.retry(&id,0);}
                            }
                        }
                    }
                    continue;
                };
                let link_lost=event.is_none();
                let result=match event {
                    Some(crate::transport::ControlEvent::Status(bytes))=>match &mut active {Some(session)=>session.status(&bytes,None),None=>Ok(())},
                    Some(crate::transport::ControlEvent::Indication(_))=>{
                        if let Some(session)=&mut active {session.cancel_uncertain();}
                        Err(crate::control::invalid("Unsolicited FTMS control indication; session closed."))
                    }
                    None=>Err(crate::session::disconnected()),
                };
                if let Err(error)=result {
                    context.bus.publish(Event::Error {data:error});
                    needs_cleanup=context.close(&mut active).await.is_err();
                    reconnect=if link_lost && context.limits.auto_reconnect {context.retry(&id,0)} else {None};
                }
            }
            _=ramp.tick(),if connected=>{
                if let Some(session)=&mut active && let Err(error)=session.advance_ramp(&context.stop).await {
                    context.bus.publish(Event::Error {data:error});
                    needs_cleanup=context.close(&mut active).await.is_err();reconnect=None;
                }
            }
            _=link.tick(),if connected=>{
                let result=tokio::select! {_=context.stop.cancelled()=>break,result=bounded(3,context.transport.is_connected())=>result};
                if !matches!(result,Ok(true)) {
                    context.bus.publish(Event::Error {data:result.err().unwrap_or_else(crate::session::disconnected)});
                    needs_cleanup=context.close(&mut active).await.is_err();
                    if context.limits.auto_reconnect {reconnect=context.retry(&id,0);}
                }
            }
        }
    }
    if active.is_some() || needs_cleanup {
        let _ = context.close(&mut active).await;
    }
}
