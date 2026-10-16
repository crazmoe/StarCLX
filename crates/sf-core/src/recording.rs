//! Gesprächsaufzeichnungen auf der Anlage (`AudioRecordService`): Liste,
//! Herunterladen, Löschen und Ereignisse für neue oder gelöschte Aufnahmen.

use std::time::Duration;

use serde::Serialize;
use sf_onehub::OneHub;
use sf_onehub::sf_proto::v1;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use v1::audiorecord as ar;

/// So viele Aufnahmen holt [`list`] höchstens
pub const LIMIT: i32 = 200;
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Wartezeit, wenn die Anlage den Dienst nicht kennt (älteres Release)
const UNIMPLEMENTED_WAIT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recording {
    pub id: String,
    /// Zugehöriger Eintrag der Rufliste, leer wenn unbekannt
    pub cdr_id: String,
    /// Anrufer und Angerufener, wie die Anlage sie beschreibt
    pub caller: String,
    pub callee: String,
    /// Nummer für den Rückruf
    pub number: String,
    /// Unix-Zeit in Millisekunden
    pub start: i64,
    pub end: i64,
    pub duration_secs: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recordings {
    /// Neueste zuerst, höchstens [`LIMIT`]
    pub list: Vec<Recording>,
    /// Anzahl auf der Anlage, kann grösser sein als `list`
    pub total: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingEvent {
    /// Etwas hat sich geändert; neu laden
    Changed,
}

fn millis(t: Option<prost_types::Timestamp>) -> i64 {
    t.map_or(0, |t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

fn view_of(r: ar::AudioRecord) -> Option<Recording> {
    let start = millis(r.audio_record_start_time);
    let end = millis(r.audio_record_end_time);
    let duration_secs = match r.audio_record_duration {
        Some(d) => d.seconds,
        None if end > start => (end - start) / 1000,
        None => 0,
    };
    Some(Recording {
        id: r.audio_record_id?.id,
        cdr_id: r.cdr_id.map(|c| c.id).unwrap_or_default(),
        caller: r.caller_info,
        callee: r.callee_info,
        number: r.call_back_number,
        start,
        end,
        duration_secs,
    })
}

fn record_id(id: &str) -> Option<ar::AudioRecordId> {
    Some(ar::AudioRecordId { id: id.to_owned() })
}

/// Die neuesten Aufnahmen, neueste zuerst.
pub async fn list(hub: &OneHub) -> sf_onehub::Result<Recordings> {
    let res = hub
        .audio_record()
        .get_audio_records(ar::GetAudioRecordsRequest {
            order_by: ar::AudioRecordOrderBy::AudioRecordStart as i32,
            order_direction: v1::types::OrderDirection::Descending as i32,
            limit: LIMIT,
            ..Default::default()
        })
        .await?
        .into_inner();
    Ok(Recordings {
        list: res.audio_records.into_iter().filter_map(view_of).collect(),
        total: res.total_count,
    })
}

/// Lädt die Aufnahme als Datei (siehe [`file_type`]).
pub async fn download(hub: &OneHub, id: &str) -> sf_onehub::Result<Vec<u8>> {
    let mut stream = hub
        .audio_record()
        .download_audio_record_file(ar::DownloadAudioRecordRequest {
            audio_record_id: record_id(id),
        })
        .await?
        .into_inner();
    let mut data = Vec::new();
    while let Some(chunk) = stream.message().await? {
        if let Some(c) = chunk.file_chunk {
            data.extend_from_slice(&c.data);
        }
    }
    Ok(data)
}

pub async fn delete(hub: &OneHub, id: &str) -> sf_onehub::Result<()> {
    hub.audio_record()
        .delete_audio_record(ar::DeleteAudioRecordRequest {
            audio_record_id: record_id(id),
        })
        .await?;
    Ok(())
}

/// Löscht mehrere Aufnahmen auf einmal.
pub async fn delete_many(hub: &OneHub, ids: &[String]) -> sf_onehub::Result<()> {
    hub.audio_record()
        .delete_audio_records(ar::DeleteAudioRecordsRequest {
            audio_record_ids: ids.iter().filter_map(|id| record_id(id)).collect(),
        })
        .await?;
    Ok(())
}

/// Dateiendung und MIME-Typ anhand der ersten Bytes. Die Anlage sagt nicht,
/// in welchem Format sie die Aufnahme liefert; ohne Erkennung gilt WAV.
pub fn file_type(data: &[u8]) -> (&'static str, &'static str) {
    match data {
        [b'I', b'D', b'3', ..] => ("mp3", "audio/mpeg"),
        [0xFF, b, ..] if b & 0xE0 == 0xE0 => ("mp3", "audio/mpeg"),
        [b'O', b'g', b'g', b'S', ..] => ("ogg", "audio/ogg"),
        _ => ("wav", "audio/wav"),
    }
}

/// Verfolgt Ereignisse zu Aufnahmen, bis es gedroppt wird.
pub struct Watcher(JoinHandle<()>);

impl Watcher {
    pub fn start(hub: OneHub, events: mpsc::UnboundedSender<RecordingEvent>) -> Self {
        Self(tokio::spawn(crate::reconnect::forever(
            "Aufzeichnungs-Ereignisse",
            MAX_BACKOFF,
            move || {
                let (hub, events) = (hub.clone(), events.clone());
                async move { watch(&hub, &events).await }
            },
        )))
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn watch(
    hub: &OneHub,
    events: &mpsc::UnboundedSender<RecordingEvent>,
) -> sf_onehub::Result<()> {
    let mut stream = match hub.audio_record().subscribe_audio_record_events(()).await {
        Ok(s) => s.into_inner(),
        Err(s) if s.code() == tonic::Code::Unimplemented => {
            tracing::debug!("Aufzeichnungs-Ereignisse: von der Anlage nicht angeboten");
            tokio::time::sleep(UNIMPLEMENTED_WAIT).await;
            return Ok(());
        }
        Err(s) => return Err(s.into()),
    };
    // Nach einem Neuverbinden kann etwas verpasst worden sein
    let _ = events.send(RecordingEvent::Changed);
    while stream.message().await?.is_some() {
        let _ = events.send(RecordingEvent::Changed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view() {
        let r = view_of(ar::AudioRecord {
            audio_record_id: record_id("42"),
            cdr_id: Some(v1::types::CdrId { id: "c7".into() }),
            caller_info: "Star2 Claude2 (12)".into(),
            callee_info: "13".into(),
            call_back_number: "12".into(),
            audio_record_start_time: Some(prost_types::Timestamp {
                seconds: 1_760_000_000,
                nanos: 500_000_000,
            }),
            audio_record_end_time: Some(prost_types::Timestamp {
                seconds: 1_760_000_065,
                nanos: 0,
            }),
            audio_record_duration: None,
        })
        .unwrap();
        assert_eq!(r.id, "42");
        assert_eq!(r.cdr_id, "c7");
        assert_eq!(r.number, "12");
        assert_eq!(r.start, 1_760_000_000_500);
        // Ohne Dauer aus Start und Ende
        assert_eq!(r.duration_secs, 64);
    }

    #[test]
    fn without_id_dropped() {
        assert_eq!(view_of(ar::AudioRecord::default()), None);
    }

    #[test]
    fn detects_file_type() {
        assert_eq!(file_type(b"RIFF\0\0\0\0WAVE").0, "wav");
        assert_eq!(file_type(b"ID3\x04").0, "mp3");
        assert_eq!(file_type(&[0xFF, 0xFB, 0x90]).0, "mp3");
        assert_eq!(file_type(b"OggS").1, "audio/ogg");
        assert_eq!(file_type(b"").0, "wav");
    }
}
