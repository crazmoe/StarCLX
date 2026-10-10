//! iQueue-Warteschlangen: wartende Anrufer, Agenten und Statistik live
//! verfolgen, einen wartenden Anruf aufs eigene Telefon holen.
//!
//! Die Queue-ID ist dieselbe wie die Gruppen-ID; an- und abmelden läuft über
//! [`crate::group::set_logged_on`]. Pause und Nachbearbeitung setzt die
//! Anlage selbst nach dem Auflegen, die Schnittstelle kann sie nur melden.
//!
//! Gemessen an einer STARFACE 10 (siehe plan/iqueue-messung.md): Viele
//! Ereignisse kommen doppelt, und beim Abonnieren schickt jeder Stream den
//! Ausgangsstand. Darum wird alles als Zustand angewandt und nur ein
//! geänderter Stand weitergegeben.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;
use sf_onehub::OneHub;
use sf_onehub::sf_proto::v1;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use v1::queue as q;

const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Agent {
    pub user_id: String,
    /// „Nachname, Vorname“
    pub name: String,
    pub logged_in: bool,
    /// Pause bzw. Nachbearbeitung nach einem Gespräch
    pub paused: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Call {
    pub id: String,
    pub caller_name: String,
    pub caller_number: String,
    /// Bei Anrufer-Priorisierung die Zahl der erfüllten Regeln, sonst 0
    pub priority: i32,
    /// Platz in der Schlange ab 1; 0 = noch nicht vergeben
    pub position: i32,
    /// "waiting", "ringing" oder "connected"
    pub state: &'static str,
    /// Eingang in Millisekunden seit 1970
    pub incoming: i64,
    /// Verbunden seit (Millisekunden), 0 = noch nicht
    pub connected: i64,
    /// Agenten, bei denen der Anruf klingelt bzw. die verbunden sind
    pub agents: Vec<String>,
    /// Angaben eines vorgeschalteten Bots, meist leer
    pub bot_output: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct Stats {
    pub callers: i32,
    pub free_agents: i32,
    pub missed: i32,
    pub unanswered: i32,
    pub total: i32,
    /// Mittlere Wartezeit in Sekunden
    pub avg_wait_secs: i32,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Queue {
    /// Zugleich die Gruppen-ID
    pub id: String,
    pub name: String,
    pub agents: Vec<Agent>,
    /// Nach Platz in der Schlange sortiert
    pub calls: Vec<Call>,
    pub stats: Stats,
}

fn queue_id(id: &str) -> Option<v1::types::QueueId> {
    Some(v1::types::QueueId { id: id.to_owned() })
}

fn id_of(id: Option<v1::types::QueueId>) -> String {
    id.map(|i| i.id).unwrap_or_default()
}

fn user_of(id: Option<v1::types::UserId>) -> String {
    id.map(|i| i.id).unwrap_or_default()
}

fn call_of(id: Option<v1::types::CallId>) -> String {
    id.map(|i| i.id).unwrap_or_default()
}

/// Die Anlage schickt vor der Annahme `Timestamp{0,0}` statt nichts.
fn millis(t: Option<prost_types::Timestamp>) -> i64 {
    t.map_or(0, |t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

fn state_name(s: i32) -> &'static str {
    match q::QueueCallState::try_from(s) {
        Ok(q::QueueCallState::Ringing) => "ringing",
        Ok(q::QueueCallState::Connected) => "connected",
        _ => "waiting",
    }
}

fn agent_view(a: q::QueueAgent) -> Agent {
    Agent {
        user_id: user_of(a.user_id),
        name: a.name,
        logged_in: a.logged_in,
        paused: a.in_queue_pause,
    }
}

fn call_view(c: q::QueueCall) -> Call {
    Call {
        id: call_of(c.call_id),
        caller_name: c.caller_name,
        caller_number: c.caller_number,
        priority: c.priority,
        position: c.position,
        state: state_name(c.queue_call_state),
        incoming: millis(c.incoming_time),
        connected: millis(c.connected_time),
        agents: c.user_ids.into_iter().map(|u| u.id).collect(),
        bot_output: c.bot_output.into_iter().collect(),
    }
}

fn stats_view(s: q::QueueStatistics) -> Stats {
    Stats {
        callers: s.callers_in_queue,
        free_agents: s.free_agents,
        missed: s.missed_calls,
        unanswered: s.unanswered_calls,
        total: s.total_calls,
        avg_wait_secs: s.average_waiting_time,
    }
}

fn queue_view(q: q::Queue) -> Queue {
    Queue {
        id: id_of(q.queue_id),
        name: q.name,
        agents: q.queue_agents.into_iter().map(agent_view).collect(),
        ..Default::default()
    }
}

/// Wartende zuerst nach Platz, ohne Platz und Verbundene dahinter
fn sort_calls(calls: &mut [Call]) {
    calls.sort_by_key(|c| {
        (
            c.state == "connected",
            c.position == 0,
            c.position,
            c.incoming,
        )
    });
}

/// Wartende Anrufer und Statistik einer Queue nachladen
async fn fill(hub: &OneHub, queue: &mut Queue) -> sf_onehub::Result<()> {
    let mut svc = hub.queue();
    let calls = svc
        .get_queue_calls(q::GetQueueCallsRequest {
            queue_id: queue_id(&queue.id),
        })
        .await?
        .into_inner()
        .queue_calls;
    queue.calls = calls.into_iter().map(call_view).collect();
    sort_calls(&mut queue.calls);
    if let Some(s) = svc
        .get_queue_statistics(q::GetQueueStatisticsRequest {
            queue_id: queue_id(&queue.id),
        })
        .await?
        .into_inner()
        .queue_statistics
    {
        queue.stats = stats_view(s);
    }
    Ok(())
}

/// Alle Queues, in denen der Benutzer Agent ist, mit Anrufen und Statistik
pub async fn list(hub: &OneHub) -> sf_onehub::Result<Vec<Queue>> {
    let queues = hub.queue().get_queues(()).await?.into_inner().queues;
    let mut out = Vec::with_capacity(queues.len());
    for q in queues {
        let mut queue = queue_view(q);
        fill(hub, &mut queue).await?;
        out.push(queue);
    }
    Ok(out)
}

/// Einen wartenden Anruf auf das Telefon `phone_id` holen
pub async fn grab(
    hub: &OneHub,
    queue: &str,
    call_id: &str,
    phone_id: &str,
) -> sf_onehub::Result<()> {
    hub.queue()
        .grab_queue_call(q::GrabQueueCallRequest {
            queue_id: queue_id(queue),
            phone_id: Some(v1::types::PhoneId {
                id: phone_id.to_owned(),
            }),
            call_id: Some(v1::types::CallId {
                id: call_id.to_owned(),
            }),
        })
        .await?;
    Ok(())
}

/// Hält die Queues aktuell; jede echte Änderung schickt den vollen Stand.
pub struct Queues(JoinHandle<()>);

impl Queues {
    pub fn start(hub: OneHub, updates: mpsc::UnboundedSender<Vec<Queue>>) -> Self {
        Self(tokio::spawn(async move {
            // Ohne iQueue oder ohne Recht gar nicht erst dauernd neu versuchen
            if let Err(e) = hub
                .queue()
                .get_queues(())
                .await
                .map_err(sf_onehub::Error::from)
                && unsupported(&e)
            {
                tracing::info!(error = %e, "Warteschlangen nicht verfügbar");
                return;
            }
            crate::reconnect::forever("Queue-Ereignisse", MAX_BACKOFF, move || {
                let (hub, updates) = (hub.clone(), updates.clone());
                async move { watch(&hub, &updates).await }
            })
            .await;
        }))
    }
}

/// Die Anlage bietet den Dienst nicht an oder der Benutzer darf ihn nicht nutzen
fn unsupported(e: &sf_onehub::Error) -> bool {
    e.permission_denied().is_some() || e.unimplemented()
}

impl Drop for Queues {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn watch(hub: &OneHub, updates: &mpsc::UnboundedSender<Vec<Queue>>) -> sf_onehub::Result<()> {
    // Erst abonnieren, dann lesen: so geht keine Änderung dazwischen verloren.
    let mut svc = hub.queue();
    let mut queue_events = svc.subscribe_queue_events(()).await?.into_inner();
    let mut call_events = svc.subscribe_queue_call_events(()).await?.into_inner();
    let mut stat_events = svc
        .subscribe_queue_statistics_events(())
        .await?
        .into_inner();
    let mut list = list(hub).await?;
    let mut sent = list.clone();
    let _ = updates.send(sent.clone());
    loop {
        tokio::select! {
            ev = queue_events.message() => {
                let Some(ev) = ev? else { return Ok(()) };
                for id in apply_queue(&mut list, ev) {
                    if let Some(queue) = list.iter_mut().find(|x| x.id == id) {
                        fill(hub, queue).await?;
                    }
                }
            }
            ev = call_events.message() => {
                let Some(ev) = ev? else { return Ok(()) };
                apply_call(&mut list, ev);
            }
            ev = stat_events.message() => {
                let Some(ev) = ev? else { return Ok(()) };
                let id = id_of(ev.queue_id);
                if let (Some(queue), Some(s)) = (list.iter_mut().find(|x| x.id == id), ev.queue_statistics) {
                    queue.stats = stats_view(s);
                }
            }
        }
        if list != sent {
            sent.clone_from(&list);
            let _ = updates.send(sent.clone());
        }
    }
}

/// Wendet ein Queue-Ereignis an; gibt neu hinzugekommene Queues zurück,
/// deren Anrufe und Statistik nachzuladen sind.
fn apply_queue(list: &mut Vec<Queue>, ev: q::QueueEventResponse) -> Vec<String> {
    use q::queue_event_response::QueueEvent as E;
    let agent = |list: &mut Vec<Queue>, queue: String, user: String, f: &dyn Fn(&mut Agent)| {
        for a in list
            .iter_mut()
            .filter(|x| x.id == queue)
            .flat_map(|x| x.agents.iter_mut())
            .filter(|a| a.user_id == user)
        {
            f(a);
        }
    };
    match ev.queue_event {
        Some(E::AddedToQueue(e)) => {
            let mut fresh = Vec::new();
            for q in e.queues {
                let q = queue_view(q);
                match list.iter_mut().find(|x| x.id == q.id) {
                    Some(old) => {
                        old.name = q.name;
                        old.agents = q.agents;
                    }
                    None => {
                        fresh.push(q.id.clone());
                        list.push(q);
                    }
                }
            }
            fresh
        }
        Some(E::RemovedFromQueue(e)) => {
            let id = id_of(e.queue_id);
            list.retain(|x| x.id != id);
            Vec::new()
        }
        Some(E::QueueNameChanged(e)) => {
            let id = id_of(e.queue_id);
            for x in list.iter_mut().filter(|x| x.id == id) {
                x.name.clone_from(&e.name);
            }
            Vec::new()
        }
        Some(E::QueueAgentAdded(e)) => {
            let id = id_of(e.queue_id);
            if let (Some(queue), Some(a)) = (list.iter_mut().find(|x| x.id == id), e.queue_agent) {
                let a = agent_view(a);
                match queue.agents.iter_mut().find(|x| x.user_id == a.user_id) {
                    Some(old) => *old = a,
                    None => queue.agents.push(a),
                }
            }
            Vec::new()
        }
        Some(E::QueueAgentRemoved(e)) => {
            let (id, user) = (id_of(e.queue_id), user_of(e.user_id));
            for x in list.iter_mut().filter(|x| x.id == id) {
                x.agents.retain(|a| a.user_id != user);
            }
            Vec::new()
        }
        Some(E::QueueAgentLoggedOn(e)) => {
            agent(list, id_of(e.queue_id), user_of(e.user_id), &|a| {
                a.logged_in = true
            });
            Vec::new()
        }
        Some(E::QueueAgentLoggedOff(e)) => {
            agent(list, id_of(e.queue_id), user_of(e.user_id), &|a| {
                a.logged_in = false
            });
            Vec::new()
        }
        // Ohne Queue-ID: gilt für den Agenten in allen Queues
        Some(E::QueueAgentPauseStateChanged(e)) => {
            let user = user_of(e.user_id);
            for a in list
                .iter_mut()
                .flat_map(|x| x.agents.iter_mut())
                .filter(|a| a.user_id == user)
            {
                a.paused = e.in_queue_pause;
            }
            Vec::new()
        }
        None => Vec::new(),
    }
}

fn apply_call(list: &mut [Queue], ev: q::QueueCallEventResponse) {
    use q::queue_call_event_response::QueueCallEvent as E;
    let Some(ev) = ev.queue_call_event else {
        return;
    };
    let id = match &ev {
        E::QueueCallAdded(e) => &e.queue_id,
        E::QueueCallRemoved(e) => &e.queue_id,
        E::QueueCallStateChanged(e) => &e.queue_id,
        E::QueueCallConnected(e) => &e.queue_id,
        E::QueueCallPositionChanged(e) => &e.queue_id,
        E::QueueCallAgentsChanged(e) => &e.queue_id,
    };
    let id = id.as_ref().map(|i| i.id.clone()).unwrap_or_default();
    let Some(queue) = list.iter_mut().find(|x| x.id == id) else {
        return;
    };
    let calls = &mut queue.calls;
    let find = |calls: &mut Vec<Call>, c: Option<v1::types::CallId>| {
        let c = call_of(c);
        calls.iter().position(|x| x.id == c)
    };
    match ev {
        E::QueueCallAdded(e) => {
            for c in e.queue_calls {
                let c = call_view(c);
                match calls.iter_mut().find(|x| x.id == c.id) {
                    Some(old) => *old = c,
                    None => calls.push(c),
                }
            }
        }
        E::QueueCallRemoved(e) => {
            if let Some(i) = find(calls, e.call_id) {
                calls.remove(i);
            }
        }
        E::QueueCallStateChanged(e) => {
            if let Some(i) = find(calls, e.call_id) {
                calls[i].state = state_name(e.queue_call_state);
            }
        }
        E::QueueCallConnected(e) => {
            if let Some(i) = find(calls, e.call_id) {
                calls[i].state = "connected";
                calls[i].connected = millis(e.connected_time);
            }
        }
        E::QueueCallPositionChanged(e) => {
            if let Some(i) = find(calls, e.call_id) {
                calls[i].position = e.position;
            }
        }
        // Die Liste ersetzt die vorige vollständig
        E::QueueCallAgentsChanged(e) => {
            if let Some(i) = find(calls, e.call_id) {
                calls[i].agents = e.user_ids.into_iter().map(|u| u.id).collect();
            }
        }
    }
    sort_calls(calls);
}

#[cfg(test)]
mod tests {
    use super::*;
    use q::queue_call_event_response::QueueCallEvent as CE;
    use q::queue_event_response::QueueEvent as QE;

    fn qid() -> Option<v1::types::QueueId> {
        queue_id("4276")
    }

    fn cid(id: &str) -> Option<v1::types::CallId> {
        Some(v1::types::CallId { id: id.into() })
    }

    fn uid(id: &str) -> Option<v1::types::UserId> {
        Some(v1::types::UserId { id: id.into() })
    }

    fn queue() -> Vec<Queue> {
        let mut list = Vec::new();
        let fresh = apply_queue(
            &mut list,
            q::QueueEventResponse {
                queue_event: Some(QE::AddedToQueue(q::AddedToQueueEvent {
                    queues: vec![q::Queue {
                        queue_id: qid(),
                        name: "Zentrale iQueue".into(),
                        queue_agents: vec![q::QueueAgent {
                            user_id: uid("me"),
                            name: "Muster, Max".into(),
                            logged_in: true,
                            in_queue_pause: false,
                        }],
                    }],
                })),
            },
        );
        assert_eq!(fresh, ["4276"]);
        list
    }

    fn call(list: &mut [Queue], ev: CE) {
        apply_call(
            list,
            q::QueueCallEventResponse {
                queue_call_event: Some(ev),
            },
        );
    }

    #[test]
    fn repeated_added_to_queue_keeps_calls() {
        let mut list = queue();
        list[0].calls.push(Call {
            id: "a".into(),
            ..Default::default()
        });
        let ev = q::QueueEventResponse {
            queue_event: Some(QE::AddedToQueue(q::AddedToQueueEvent {
                queues: vec![q::Queue {
                    queue_id: qid(),
                    name: "Neu".into(),
                    queue_agents: vec![],
                }],
            })),
        };
        assert!(apply_queue(&mut list, ev).is_empty());
        assert_eq!(list[0].name, "Neu");
        assert_eq!(list[0].calls.len(), 1);
    }

    #[test]
    fn call_lifecycle_as_measured() {
        let mut list = queue();
        call(
            &mut list,
            CE::QueueCallAdded(q::QueueCallAddedEvent {
                queue_id: qid(),
                queue_calls: vec![q::QueueCall {
                    call_id: cid("a"),
                    caller_name: "Anrufer".into(),
                    queue_call_state: q::QueueCallState::Waiting as i32,
                    incoming_time: Some(prost_types::Timestamp {
                        seconds: 100,
                        nanos: 0,
                    }),
                    connected_time: Some(prost_types::Timestamp::default()),
                    ..Default::default()
                }],
            }),
        );
        let c = &list[0].calls[0];
        assert_eq!(
            (c.state, c.position, c.incoming, c.connected),
            ("waiting", 0, 100_000, 0)
        );

        call(
            &mut list,
            CE::QueueCallPositionChanged(q::QueueCallPositionChangedEvent {
                queue_id: qid(),
                call_id: cid("a"),
                position: 1,
            }),
        );
        call(
            &mut list,
            CE::QueueCallStateChanged(q::QueueCallStateChangedEvent {
                queue_id: qid(),
                call_id: cid("a"),
                queue_call_state: q::QueueCallState::Ringing as i32,
            }),
        );
        call(
            &mut list,
            CE::QueueCallAgentsChanged(q::QueueCallAgentsChangedEvent {
                queue_id: qid(),
                call_id: cid("a"),
                user_ids: vec![v1::types::UserId { id: "me".into() }],
            }),
        );
        let c = &list[0].calls[0];
        assert_eq!(
            (c.state, c.position, c.agents.as_slice()),
            ("ringing", 1, ["me".to_owned()].as_slice())
        );

        call(
            &mut list,
            CE::QueueCallConnected(q::QueueCallConnectedEvent {
                queue_id: qid(),
                call_id: cid("a"),
                connected_time: Some(prost_types::Timestamp {
                    seconds: 110,
                    nanos: 0,
                }),
            }),
        );
        assert_eq!(
            (list[0].calls[0].state, list[0].calls[0].connected),
            ("connected", 110_000)
        );

        call(
            &mut list,
            CE::QueueCallRemoved(q::QueueCallRemovedEvent {
                queue_id: qid(),
                call_id: cid("a"),
            }),
        );
        assert!(list[0].calls.is_empty());
    }

    #[test]
    fn pause_applies_to_agent_in_every_queue() {
        let mut list = queue();
        let mut other = list[0].clone();
        other.id = "9".into();
        list.push(other);
        apply_queue(
            &mut list,
            q::QueueEventResponse {
                queue_event: Some(QE::QueueAgentPauseStateChanged(
                    q::QueueAgentPauseStateChangedEvent {
                        user_id: uid("me"),
                        in_queue_pause: true,
                    },
                )),
            },
        );
        assert!(list.iter().all(|x| x.agents[0].paused));
    }

    #[test]
    fn logged_off_only_in_named_queue() {
        let mut list = queue();
        apply_queue(
            &mut list,
            q::QueueEventResponse {
                queue_event: Some(QE::QueueAgentLoggedOff(q::QueueAgentLoggedOffEvent {
                    queue_id: qid(),
                    user_id: uid("me"),
                })),
            },
        );
        assert!(!list[0].agents[0].logged_in);
    }

    #[test]
    fn waiting_calls_sorted_by_position() {
        let c = |id: &str, position, state| Call {
            id: id.into(),
            position,
            state,
            ..Default::default()
        };
        let mut calls = vec![
            c("conn", 0, "connected"),
            c("new", 0, "waiting"),
            c("second", 2, "waiting"),
            c("first", 1, "ringing"),
        ];
        sort_calls(&mut calls);
        let ids: Vec<_> = calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["first", "second", "new", "conn"]);
    }
}
