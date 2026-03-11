// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! Project Helix-Core v5.0 — Rust Edition
//!
//! High-performance DNA Data Storage OS server.
//! Full pipeline: Compress → RS → Fountain → Transcode → OligoBuilder → Constraints → FASTA → Cost

#![recursion_limit = "512"]

use actix_cors::Cors;
use actix_files as fs;
use actix_multipart::Multipart;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use base64::Engine;
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;
use actix_web::middleware::Logger;
use log::info;
use tokio::sync::broadcast;

use helix_core::pipeline::{HelixPipeline, PipelineConfig};

// ========== App State ==========

const MAX_UPLOAD_BYTES: usize = 500 * 1024 * 1024; // 500 MB
const SESSION_TTL_SECS: u64 = 1800; // 30 minutes
const MAX_IMAGE_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct SessionState {
    pipeline: Arc<Mutex<HelixPipeline>>,
    original_data: Arc<Mutex<Option<Vec<u8>>>>,
    original_filename: Arc<Mutex<Option<String>>>,
    last_active: Arc<Mutex<SystemTime>>,
}

struct AppState {
    tasks: RwLock<HashMap<String, TaskState>>,
    sessions: RwLock<HashMap<String, SessionState>>,
    sse_channels: RwLock<HashMap<String, broadcast::Sender<TaskState>>>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskState {
    id: String,
    #[serde(skip_serializing)]
    owner: String,
    status: String,
    phase: String,
    percent: u32,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

impl TaskState {
    fn new(id: &str, owner: &str) -> Self {
        Self {
            id: id.to_string(),
            owner: owner.to_string(),
            status: "running".to_string(),
            phase: "Starting...".to_string(),
            percent: 0,
            result: None,
            error: None,
        }
    }
}

fn client_key(req: &HttpRequest) -> String {
    let sanitize_sid = |raw: &str| -> Option<String> {
        let trimmed = raw.trim();
        let valid = !trimmed.is_empty()
            && trimmed.len() <= 128
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if valid {
            Some(trimmed.to_string())
        } else {
            None
        }
    };

    if let Some(session_header) = req
        .headers()
        .get("x-helix-session")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(sid) = sanitize_sid(session_header) {
            return format!("sid:{sid}");
        }
    }
    if let Some(sid) = req.query_string().split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        if key == "sid" {
            sanitize_sid(value)
        } else {
            None
        }
    }) {
        return format!("sid:{sid}");
    }

    let ip = req
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| "unknown_ip".to_string());
    let ua = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown_ua");
    format!("{ip}|{ua}")
}

/// Sanitize filename for use in HTTP Content-Disposition headers.
/// Strips anything that isn't alphanumeric, dot, dash, or underscore.
fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "file".to_string()
    } else {
        sanitized
    }
}

fn get_or_create_session(state: &AppState, key: &str) -> SessionState {
    // Deterministic purge: always purge when session count exceeds threshold
    {
        let should_purge = state.sessions.read().map(|s| s.len() > 100).unwrap_or(false);
        if should_purge {
            if let Ok(mut sessions) = state.sessions.write() {
                let now = SystemTime::now();
                sessions.retain(|_, sess| {
                    if let Ok(last) = sess.last_active.lock() {
                        now.duration_since(*last).unwrap_or_default().as_secs() < SESSION_TTL_SECS
                    } else {
                        false
                    }
                });
            }
        }
    }

    if let Ok(sessions) = state.sessions.read() {
        if let Some(sess) = sessions.get(key) {
            // Update last_active timestamp
            if let Ok(mut ts) = sess.last_active.lock() {
                *ts = SystemTime::now();
            }
            return sess.clone();
        }
    }

    let mut sessions = state.sessions.write().expect("sessions RwLock poisoned");
    if let Some(sess) = sessions.get(key) {
        if let Ok(mut ts) = sess.last_active.lock() {
            *ts = SystemTime::now();
        }
        return sess.clone();
    }

    let session = SessionState {
        pipeline: Arc::new(Mutex::new(HelixPipeline::new(PipelineConfig::default()))),
        original_data: Arc::new(Mutex::new(None)),
        original_filename: Arc::new(Mutex::new(None)),
        last_active: Arc::new(Mutex::new(SystemTime::now())),
    };
    sessions.insert(key.to_string(), session.clone());
    session
}

fn new_task(state: &AppState, owner: &str) -> String {
    // 16 hex chars (64 bits entropy) avoids birthday collisions up to ~2^32 concurrent tasks
    let id = uuid::Uuid::new_v4().to_string().replace('-', "")[..16].to_string();
    let mut tasks = state.tasks.write().expect("tasks RwLock poisoned");

    // Purge completed/errored tasks older than 50 entries to prevent memory leak
    if tasks.len() > 50 {
        let done_ids: Vec<String> = tasks
            .iter()
            .filter(|(_, t)| t.status == "done" || t.status == "error")
            .map(|(k, _)| k.clone())
            .collect();
        for old_id in done_ids.iter().take(done_ids.len().saturating_sub(5)) {
            tasks.remove(old_id);
        }
        // Also purge corresponding SSE channels
        if let Ok(mut channels) = state.sse_channels.write() {
            for old_id in done_ids.iter().take(done_ids.len().saturating_sub(5)) {
                channels.remove(old_id);
            }
        }
    }

    tasks.insert(id.clone(), TaskState::new(&id, owner));
    drop(tasks);

    // Create SSE broadcast channel for this task (buffer 128 events)
    let (tx, _) = broadcast::channel(128);
    state.sse_channels.write().expect("sse_channels RwLock poisoned").insert(id.clone(), tx);

    id
}

fn update_task(state: &AppState, id: &str, f: impl FnOnce(&mut TaskState)) {
    // FIX: Release tasks write lock BEFORE acquiring sse_channels read lock
    // to prevent deadlock from inconsistent lock ordering.
    let snapshot = {
        let mut tasks = match state.tasks.write() {
            Ok(t) => t,
            Err(_) => return,
        };
        if let Some(t) = tasks.get_mut(id) {
            f(t);
            Some(t.clone())
        } else {
            None
        }
    }; // tasks write lock dropped here

    // Now safe to acquire sse_channels read lock
    if let Some(snapshot) = snapshot {
        if let Ok(channels) = state.sse_channels.read() {
            if let Some(tx) = channels.get(id) {
                let _ = tx.send(snapshot);
            }
        }
    }
}

// ========== Helpers ==========

fn is_image_file(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico"
    )
}

fn get_mime_type(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

fn corrupt_bytes_for_preview(data: &[u8], rate: f64) -> Vec<u8> {
    use rand::prelude::*;
    use rand::rngs::StdRng;
    let mut rng = StdRng::seed_from_u64(42);
    let mut arr = data.to_vec();
    let header_safe = 200.min(arr.len());
    let n = ((arr.len() - header_safe) as f64 * rate) as usize;
    for _ in 0..n.min(arr.len() - header_safe) {
        let pos = rng.gen_range(header_safe..arr.len());
        arr[pos] = rng.gen();
    }
    arr
}

fn compute_dna_analytics(sequence: &str) -> serde_json::Value {
    if sequence.is_empty() {
        return serde_json::json!({});
    }

    let bytes = sequence.as_bytes();
    let mut counts = [0u64; 4]; // A=0, C=1, G=2, T=3

    for &b in bytes {
        match b {
            b'A' | b'a' => counts[0] += 1,
            b'C' | b'c' => counts[1] += 1,
            b'G' | b'g' => counts[2] += 1,
            b'T' | b't' => counts[3] += 1,
            _ => {}
        }
    }
    let total = counts.iter().sum::<u64>().max(1) as f64;

    // GC windows — work directly on bytes
    let window = 50usize;
    let sample_len = bytes.len().min(5000);
    let mut gc_windows = Vec::new();
    let mut i = 0;
    while i + window <= sample_len {
        let gc = bytes[i..i + window]
            .iter()
            .filter(|&&c| c == b'G' || c == b'C')
            .count() as f64
            / window as f64;
        gc_windows.push((gc * 1000.0).round() / 1000.0);
        i += window / 2;
    }

    // Dinucleotide freq — byte-level
    let mut dinucs: HashMap<[u8; 2], usize> = HashMap::new();
    let di_limit = bytes.len().min(5000);
    for i in 0..di_limit.saturating_sub(1) {
        let pair = [bytes[i], bytes[i + 1]];
        if pair.iter().all(|&b| matches!(b, b'A' | b'C' | b'G' | b'T')) {
            *dinucs.entry(pair).or_insert(0) += 1;
        }
    }
    let mut dinuc_vec: Vec<_> = dinucs.into_iter()
        .map(|(k, v)| (String::from_utf8_lossy(&k).to_string(), v))
        .collect();
    dinuc_vec.sort_by(|a, b| b.1.cmp(&a.1));
    let dinuc_map: HashMap<String, usize> =
        dinuc_vec.into_iter().take(16).collect();

    // Codon freq — byte-level
    let mut codons: HashMap<[u8; 3], usize> = HashMap::new();
    let codon_limit = bytes.len().min(6000);
    let mut j = 0;
    while j + 3 <= codon_limit {
        let codon = [bytes[j], bytes[j + 1], bytes[j + 2]];
        if codon.iter().all(|&b| matches!(b, b'A' | b'C' | b'G' | b'T')) {
            *codons.entry(codon).or_insert(0) += 1;
        }
        j += 3;
    }
    let mut codon_vec: Vec<_> = codons.into_iter()
        .map(|(k, v)| (String::from_utf8_lossy(&k).to_string(), v))
        .collect();
    codon_vec.sort_by(|a, b| b.1.cmp(&a.1));
    let codon_map: HashMap<String, usize> =
        codon_vec.into_iter().take(20).collect();

    // Runs — byte-level
    let mut longest_run = 0usize;
    let mut run_dist: HashMap<usize, usize> = HashMap::new();
    let run_limit = bytes.len().min(10000);
    if run_limit > 0 {
        let mut cur = bytes[0];
        let mut run = 1;
        for &c in bytes[1..run_limit].iter() {
            if c == cur {
                run += 1;
            } else {
                if run >= 2 {
                    *run_dist.entry(run).or_insert(0) += 1;
                    longest_run = longest_run.max(run);
                }
                cur = c;
                run = 1;
            }
        }
        if run >= 2 {
            *run_dist.entry(run).or_insert(0) += 1;
            longest_run = longest_run.max(run);
        }
    }

    let gc = (counts[2] + counts[1]) as f64 / total;

    serde_json::json!({
        "base_counts": {
            "A": counts[0], "C": counts[1],
            "G": counts[2], "T": counts[3],
        },
        "base_frequency": {
            "A": (counts[0] as f64 / total * 10000.0).round() / 10000.0,
            "C": (counts[1] as f64 / total * 10000.0).round() / 10000.0,
            "G": (counts[2] as f64 / total * 10000.0).round() / 10000.0,
            "T": (counts[3] as f64 / total * 10000.0).round() / 10000.0,
        },
        "gc_content": (gc * 10000.0).round() / 10000.0,
        "at_content": ((1.0 - gc) * 10000.0).round() / 10000.0,
        "total_bases": total as u64,
        "gc_window_data": gc_windows,
        "dinucleotide_freq": dinuc_map,
        "codon_freq": codon_map,
        "homopolymer_runs": run_dist,
        "longest_run": longest_run,
        "sequence_preview": &sequence[..sequence.len().min(3000)],
    })
}

// ========== Route: Progress (legacy polling — kept for compatibility) ==========

async fn api_progress(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let task_id = path.into_inner();
    let owner = client_key(&req);
    match state.tasks.read() {
        Ok(tasks) => match tasks.get(&task_id) {
            Some(t) if t.owner == owner => HttpResponse::Ok().json(t),
            None => HttpResponse::NotFound().json(serde_json::json!({"status": "not_found"})),
            Some(_) => HttpResponse::NotFound().json(serde_json::json!({"status": "not_found"})),
        },
        Err(_) => HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": "Internal state error"})),
    }
}

// ========== Route: SSE (Server-Sent Events — replaces polling) ==========

async fn api_events(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let task_id = path.into_inner();
    let owner = client_key(&req);

    // Verify task exists and belongs to this client
    let initial_state = {
        match state.tasks.read() {
            Ok(tasks) => match tasks.get(&task_id) {
                Some(t) if t.owner == owner => Some(t.clone()),
                _ => None,
            },
            Err(_) => None,
        }
    };

    let initial_state = match initial_state {
        Some(s) => s,
        None => return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Task not found"})),
    };

    // If task already finished, return a single SSE event and close
    if initial_state.status == "done" || initial_state.status == "error" {
        let data = serde_json::to_string(&initial_state).unwrap_or_default();
        return HttpResponse::Ok()
            .insert_header(("Content-Type", "text/event-stream"))
            .insert_header(("Cache-Control", "no-cache"))
            .insert_header(("X-Accel-Buffering", "no"))
            .body(format!("data: {}\n\n", data));
    }

    // Subscribe to SSE broadcast channel
    let rx = {
        match state.sse_channels.read() {
            Ok(channels) => channels.get(&task_id).map(|tx| tx.subscribe()),
            Err(_) => None,
        }
    };

    let rx = match rx {
        Some(r) => r,
        None => return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "SSE channel not found"})),
    };

    // Stream SSE events using unfold
    // State: (rx, initial_state_option, sent_initial, stream_done)
    let stream = futures_util::stream::unfold(
        (rx, Some(initial_state), false, false),
        |(mut rx, initial, sent_initial, stream_done)| async move {
            if stream_done {
                return None; // Close the stream
            }

            // First event: send current state so client catches up
            if !sent_initial {
                if let Some(state) = initial {
                    let data = serde_json::to_string(&state).unwrap_or_default();
                    let bytes = actix_web::web::Bytes::from(format!("data: {}\n\n", data));
                    let done = state.status == "done" || state.status == "error";
                    return Some((Ok::<_, actix_web::Error>(bytes), (rx, None, true, done)));
                }
            }

            // Listen for broadcast updates
            match rx.recv().await {
                Ok(state) => {
                    let done = state.status == "done" || state.status == "error";
                    let data = serde_json::to_string(&state).unwrap_or_default();
                    let bytes = actix_web::web::Bytes::from(format!("data: {}\n\n", data));
                    Some((Ok(bytes), (rx, None, true, done)))
                }
                Err(broadcast::error::RecvError::Closed) => None,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Missed some events, send a keepalive and continue
                    let bytes = actix_web::web::Bytes::from(": keepalive\n\n");
                    Some((Ok(bytes), (rx, None, true, false)))
                }
            }
        },
    );

    HttpResponse::Ok()
        .insert_header(("Content-Type", "text/event-stream"))
        .insert_header(("Cache-Control", "no-cache"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(stream)
}

// ========== Route: Encode ==========

#[derive(Deserialize)]
struct TextInput {
    text: Option<String>,
    redundancy: Option<f64>,
}

async fn api_encode(
    req: HttpRequest,
    payload: web::Payload,
    state: web::Data<AppState>,
) -> HttpResponse {
    let sid = client_key(&req);
    info!("api_encode started for session {sid}");
    let session = get_or_create_session(state.get_ref(), &sid);

    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let (data, filename, redundancy) = if ct.contains("multipart") {
        match parse_multipart(
            Multipart::new(req.headers(), payload),
        ).await
        {
            Ok(v) => v,
            Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({"error": e})),
        }
    } else {
        // JSON body
        let mut body = Vec::<u8>::new();
        let mut stream = payload;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => {
                    if body.len() + c.len() > MAX_UPLOAD_BYTES {
                        return HttpResponse::PayloadTooLarge()
                            .json(serde_json::json!({"error": "Payload too large"}));
                    }
                    body.extend_from_slice(&c);
                }
                Err(e) => {
                    return HttpResponse::BadRequest()
                        .json(serde_json::json!({"error": e.to_string()}))
                }
            }
        }
        let input: TextInput = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return HttpResponse::BadRequest()
                    .json(serde_json::json!({"error": e.to_string()}))
            }
        };
        let text = input.text.unwrap_or_default();
        if text.trim().is_empty() {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "No text provided."}));
        }
        (
            text.into_bytes(),
            "text_input.txt".to_string(),
            input.redundancy.unwrap_or(2.0),
        )
    };

    if data.is_empty() {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "Empty data!"}));
    }
    if !redundancy.is_finite() || redundancy <= 0.0 || redundancy > 20.0 {
        return HttpResponse::BadRequest().json(
            serde_json::json!({"error": "Invalid redundancy. Allowed range: (0, 20]."}),
        );
    }

    // Store original
    *session.original_data.lock().expect("original_data lock poisoned") = Some(data.clone());
    *session.original_filename.lock().expect("original_filename lock poisoned") = Some(filename.clone());

    // Always update redundancy to match request
    {
        let mut pipeline = session.pipeline.lock().expect("pipeline lock poisoned");
        pipeline.update_config(&serde_json::json!({"redundancy": redundancy}));
    }

    let task_id = new_task(state.get_ref(), &sid);

    // Spawn blocking task
    let state_clone = state.into_inner().clone();
    let tid = task_id.clone();
    let fname = filename.clone();
    let data_for_task = data.clone();
    let session_for_task = session.clone();

    tokio::task::spawn_blocking(move || {
        let cb_state = state_clone.clone();
        let cb_tid = tid.clone();
        let cb = move |phase: &str, pct: u32| {
            update_task(&cb_state, &cb_tid, |t| {
                t.phase = phase.to_string();
                t.percent = pct;
            });
        };

        let result = {
            let mut pipeline = session_for_task.pipeline.lock().expect("pipeline lock poisoned");
            pipeline.encode(&data_for_task, &fname, Some(&cb))
        };

        // Build response JSON
        let is_img = is_image_file(&fname);
        let image_b64 = if is_img && data_for_task.len() <= MAX_IMAGE_PREVIEW_BYTES {
            let eng = base64::engine::general_purpose::STANDARD;
            Some(format!(
                "data:{};base64,{}",
                get_mime_type(&fname),
                eng.encode(&data_for_task)
            ))
        } else {
            None
        };

        let full_seq = {
            let pipeline = session_for_task.pipeline.lock().expect("pipeline lock poisoned");
            pipeline.get_full_dna_sequence().to_string()
        };
        let analytics = compute_dna_analytics(&full_seq);

        // FIX: Extract block expressions from json! macro (they cause compile errors)
        let fasta_size_bytes = {
            let pipeline = session_for_task.pipeline.lock().expect("pipeline lock poisoned");
            pipeline.last_encode.as_ref().map(|e| e.full_fasta_content.len()).unwrap_or(0)
        };
        let uncompressed_fasta_estimate = {
            let rs_factor_num = 255usize;
            let rs_factor_den = 223usize;
            let oligo_total = result.oligo_build_stats.as_ref().map(|b| b.oligo_total_length).unwrap_or(300);
            let payload_len = result.oligo_build_stats.as_ref().map(|b| b.payload_length).unwrap_or(228);
            if payload_len > 0 {
                result.original_size * 4 * rs_factor_num / rs_factor_den * oligo_total / payload_len
            } else { 0usize }
        };

        let response = serde_json::json!({
            "success": true,
            "filename": fname,
            "is_image": is_img,
            "image_preview": image_b64,
            "original_size": result.original_size,
            "original_size_bytes": result.original_size,
            "original_checksum": result.original_checksum,
            "dna_sequence_preview": result.dna_sequence_preview,
            "dna_length": result.transcode.sequence_length,
            "gc_content": result.transcode.gc_content,
            "homopolymer_safe": result.transcode.homopolymer_safe,
            "rotation_key": result.transcode.rotation_key,
            "num_blocks": result.fountain_stats.num_blocks,
            "num_droplets": result.fountain_stats.num_droplets,
            "redundancy_ratio": result.fountain_stats.redundancy_ratio,
            "overhead_percent": result.fountain_stats.overhead_percent,
            "total_encoded_size": result.fountain_stats.total_encoded_size,
            "fasta_content": result.fasta_content,
            "fasta_stats": result.fasta_stats,
            "num_oligos": result.num_oligos,
            "encode_time": result.encode_time,
            "analytics": analytics,
            "compression_enabled": result.compression_enabled,
            "compression_stats": result.compression_stats,
            "pre_compress_size": result.pre_compress_size,
            "post_compress_size": result.post_compress_size,
            "pre_compress_size_bytes": result.pre_compress_size,
            "post_compress_size_bytes": result.post_compress_size,
            "rs_stats": result.rs_stats,
            "constraint_report": result.constraint_report,
            "oligo_quality": result.oligo_quality,
            "oligo_build_stats": result.oligo_build_stats,
            "cost_estimate": result.cost_estimate,
            "fasta_size_bytes": fasta_size_bytes,
            "uncompressed_fasta_estimate": uncompressed_fasta_estimate,
        });

        update_task(&state_clone, &tid, |t| {
            t.status = "done".to_string();
            t.percent = 100;
            t.phase = "Complete".to_string();
            t.result = Some(response);
        });
    });

    HttpResponse::Ok().json(serde_json::json!({"task_id": task_id}))
}

// ========== Route: Chaos ==========

#[derive(Deserialize)]
struct ChaosInput {
    loss_rate: Option<f64>,
    deletion_rate: Option<f64>,
    substitution_rate: Option<f64>,
    insertion_rate: Option<f64>,
}

async fn api_chaos(
    req: HttpRequest,
    body: web::Json<ChaosInput>,
    state: web::Data<AppState>,
) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);

    let loss_rate = body.loss_rate.unwrap_or(0.30);
    let del = body.deletion_rate;
    let sub = body.substitution_rate;
    let ins = body.insertion_rate;
    if !(0.0..=1.0).contains(&loss_rate)
        || del.is_some_and(|v| !(0.0..=1.0).contains(&v))
        || sub.is_some_and(|v| !(0.0..=1.0).contains(&v))
        || ins.is_some_and(|v| !(0.0..=1.0).contains(&v))
    {
        return HttpResponse::BadRequest().json(
            serde_json::json!({"error": "All chaos rates must be in [0, 1]."}),
        );
    }

    let task_id = new_task(state.get_ref(), &owner_key);
    let state_clone = state.into_inner().clone();
    let tid = task_id.clone();
    let session_for_task = session.clone();

    let orig_data = session.original_data.lock().expect("lock poisoned").clone();
    let orig_fname = session
        .original_filename
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();

    tokio::task::spawn_blocking(move || {
        let cb_state = state_clone.clone();
        let cb_tid = tid.clone();
        let cb = move |phase: &str, pct: u32| {
            update_task(&cb_state, &cb_tid, |t| {
                t.phase = phase.to_string();
                t.percent = pct;
            });
        };

        let result = {
            let mut pipeline = session_for_task.pipeline.lock().expect("lock poisoned");
            info!("task {} : apply_chaos API endpoint", tid);
            pipeline.apply_chaos(loss_rate, del, sub, ins, Some(&cb))
        };

        match result {
            Ok(chaos_out) => {
                let is_img = is_image_file(&orig_fname);
                let corrupted_b64 = if is_img {
                    if let Some(ref data) = orig_data {
                        if data.len() > MAX_IMAGE_PREVIEW_BYTES {
                            None
                        } else {
                        let total_corr = loss_rate * 0.5
                            + del.unwrap_or(0.15) * 0.3
                            + sub.unwrap_or(0.05) * 0.3;
                        let corrupted =
                            corrupt_bytes_for_preview(data, total_corr.min(0.5));
                        let eng = base64::engine::general_purpose::STANDARD;
                        Some(format!(
                            "data:{};base64,{}",
                            get_mime_type(&orig_fname),
                            eng.encode(&corrupted)
                        ))
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let mutated_analytics = compute_dna_analytics(
                    &chaos_out.mutated_sequence_preview,
                );

                let response = serde_json::json!({
                    "success": true,
                    "is_image": is_img,
                    "corrupted_image": corrupted_b64,
                    "chaos_stats": chaos_out.chaos_stats,
                    "mutation_summary": chaos_out.mutation_summary,
                    "dna_mutation_affects_decode": chaos_out.dna_mutation_affects_decode,
                    "original_sequence_preview": chaos_out.original_sequence_preview,
                    "mutated_sequence_preview": chaos_out.mutated_sequence_preview,
                    "droplet_survival_rate": chaos_out.droplet_survival_rate,
                    "chaos_time": chaos_out.chaos_time,
                    "mutated_analytics": mutated_analytics,
                });

                update_task(&state_clone, &tid, |t| {
                    t.status = "done".to_string();
                    t.percent = 100;
                    t.phase = "Complete".to_string();
                    t.result = Some(response);
                });
            }
            Err(e) => {
                update_task(&state_clone, &tid, |t| {
                    t.status = "error".to_string();
                    t.error = Some(e);
                });
            }
        }
    });

    HttpResponse::Ok().json(serde_json::json!({"task_id": task_id}))
}

// ========== Route: Decode ==========

async fn api_decode(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);

    let task_id = new_task(state.get_ref(), &owner_key);
    let state_clone = state.into_inner().clone();
    let tid = task_id.clone();
    let session_for_task = session.clone();

    let orig_data = session.original_data.lock().expect("original_data lock poisoned").clone();
    let orig_fname = session
        .original_filename
        .lock()
        .expect("original_filename lock poisoned")
        .clone()
        .unwrap_or_default();

    tokio::task::spawn_blocking(move || {
        let cb_state = state_clone.clone();
        let cb_tid = tid.clone();
        let cb = move |phase: &str, pct: u32| {
            update_task(&cb_state, &cb_tid, |t| {
                t.phase = phase.to_string();
                t.percent = pct;
            });
        };

        let result = {
            let mut pipeline = session_for_task.pipeline.lock().expect("lock poisoned");
            pipeline.decode(Some(&cb))
        };

        match result {
            Ok(dec) => {
                let is_img = is_image_file(&orig_fname);
                let recovered_b64 = if is_img {
                    dec.recovered_data.as_ref().and_then(|bytes| {
                        if bytes.len() > MAX_IMAGE_PREVIEW_BYTES {
                            None
                        } else {
                            let eng = base64::engine::general_purpose::STANDARD;
                            Some(format!(
                                "data:{};base64,{}",
                                get_mime_type(&orig_fname),
                                eng.encode(bytes)
                            ))
                        }
                    })
                } else {
                    None
                };

                let response = serde_json::json!({
                    "success": dec.success,
                    "data_match": dec.data_match,
                    "is_image": is_img,
                    "recovered_image": recovered_b64,
                    "recovered_size": dec.recovered_size,
                    "recovered_preview": dec.recovered_preview,
                    "decode_stats": dec.decode_stats,
                    "decode_time": dec.decode_time,
                    "original_size": orig_data.as_ref().map(|d| d.len()).unwrap_or(0),
                    "filename": orig_fname,
                    "decompression_stats": dec.decompression_stats,
                    "rs_correction_stats": dec.rs_correction_stats,
                });

                update_task(&state_clone, &tid, |t| {
                    t.status = "done".to_string();
                    t.percent = 100;
                    t.phase = "Complete".to_string();
                    t.result = Some(response);
                });
            }
            Err(e) => {
                update_task(&state_clone, &tid, |t| {
                    t.status = "error".to_string();
                    t.error = Some(e);
                });
            }
        }
    });

    HttpResponse::Ok().json(serde_json::json!({"task_id": task_id}))
}

// ========== Download routes ==========

async fn api_decode_fasta(
    req: HttpRequest,
    payload: web::Payload,
    state: web::Data<AppState>,
) -> HttpResponse {
    let sid = client_key(&req);
    info!("api_decode_fasta started for session {sid}");
    let session = get_or_create_session(state.get_ref(), &sid);

    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let fasta_content = if ct.contains("multipart") {
        match parse_multipart(Multipart::new(req.headers(), payload)).await {
            Ok((data, _filename, _redundancy)) => {
                match String::from_utf8(data) {
                    Ok(s) => s,
                    Err(_) => return HttpResponse::BadRequest()
                        .json(serde_json::json!({"error": "FASTA file must be valid UTF-8 text"})),
                }
            }
            Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({"error": e})),
        }
    } else {
        // Raw body or JSON with fasta_content field
        let mut body = Vec::<u8>::new();
        let mut stream = payload;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => {
                    if body.len() + c.len() > MAX_UPLOAD_BYTES {
                        return HttpResponse::PayloadTooLarge()
                            .json(serde_json::json!({"error": "Payload too large"}));
                    }
                    body.extend_from_slice(&c);
                }
                Err(e) => {
                    return HttpResponse::BadRequest()
                        .json(serde_json::json!({"error": e.to_string()}))
                }
            }
        }
        // Try JSON first
        if let Ok(json_input) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(fc) = json_input.get("fasta_content").and_then(|v| v.as_str()) {
                fc.to_string()
            } else {
                String::from_utf8(body).unwrap_or_default()
            }
        } else {
            String::from_utf8(body).unwrap_or_default()
        }
    };

    if fasta_content.trim().is_empty() {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "Empty FASTA content"}));
    }

    let task_id = new_task(state.get_ref(), &sid);
    let state_clone = state.into_inner().clone();
    let tid = task_id.clone();
    let session_for_task = session.clone();

    tokio::task::spawn_blocking(move || {
        let cb_state = state_clone.clone();
        let cb_tid = tid.clone();
        let cb = move |phase: &str, pct: u32| {
            update_task(&cb_state, &cb_tid, |t| {
                t.phase = phase.to_string();
                t.percent = pct;
            });
        };

        let result = {
            let mut pipeline = session_for_task.pipeline.lock().expect("lock poisoned");
            pipeline.decode_from_fasta(&fasta_content, Some(&cb))
        };

        match result {
            Ok(dec) => {
                let is_img = is_image_file(&dec.original_filename);
                let recovered_b64 = if is_img {
                    dec.recovered_data.as_ref().and_then(|bytes| {
                        if bytes.len() > MAX_IMAGE_PREVIEW_BYTES {
                            None
                        } else {
                            let eng = base64::engine::general_purpose::STANDARD;
                            Some(format!(
                                "data:{};base64,{}",
                                get_mime_type(&dec.original_filename),
                                eng.encode(bytes)
                            ))
                        }
                    })
                } else {
                    None
                };

                // Detect file type for preview
                let file_type_info = dec.recovered_data.as_ref().map(|data| {
                    detect_file_type_info(data, &dec.original_filename)
                });

                // Store recovered data for download
                if let Some(ref data) = dec.recovered_data {
                    *session_for_task.original_data.lock().expect("lock poisoned") = Some(data.clone());
                    *session_for_task.original_filename.lock().expect("lock poisoned") = Some(dec.original_filename.clone());
                }

                let response = serde_json::json!({
                    "success": dec.success,
                    "data_match": dec.data_match,
                    "is_image": is_img,
                    "recovered_image": recovered_b64,
                    "recovered_size": dec.recovered_size,
                    "recovered_preview": dec.recovered_preview,
                    "original_filename": dec.original_filename,
                    "original_checksum": dec.original_checksum,
                    "actual_checksum": dec.actual_checksum,
                    "num_oligos_parsed": dec.num_oligos_parsed,
                    "crc_pass": dec.crc_pass,
                    "crc_fail": dec.crc_fail,
                    "decode_time": dec.decode_time,
                    "decompression_stats": dec.decompression_stats,
                    "rs_correction_stats": dec.rs_correction_stats,
                    "file_type_info": file_type_info,
                });

                update_task(&state_clone, &tid, |t| {
                    t.status = "done".to_string();
                    t.percent = 100;
                    t.phase = "Complete".to_string();
                    t.result = Some(response);
                });
            }
            Err(e) => {
                update_task(&state_clone, &tid, |t| {
                    t.status = "error".to_string();
                    t.error = Some(e);
                });
            }
        }
    });

    HttpResponse::Ok().json(serde_json::json!({"task_id": task_id}))
}

// ========== Route: Benchmark ==========

async fn api_benchmark(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> HttpResponse {
    let sid = client_key(&req);
    let task_id = new_task(state.get_ref(), &sid);
    let state_clone = state.into_inner().clone();
    let tid = task_id.clone();

    tokio::task::spawn_blocking(move || {
        let cb_state = state_clone.clone();
        let cb_tid = tid.clone();
        let cb = move |phase: &str, pct: u32| {
            update_task(&cb_state, &cb_tid, |t| {
                t.phase = phase.to_string();
                t.percent = pct;
            });
        };

        let mut results = Vec::new();

        let test_cases: Vec<(&str, Vec<u8>)> = vec![
            ("Text (1KB)", "The quick brown fox jumps over the lazy dog. ".repeat(23).into_bytes()),
            ("Text (10KB)", "DNA storage is the future of long-term archival data preservation. ".repeat(152).into_bytes()),
            ("CSV data", "id,name,value,timestamp\n1,alpha,42.5,2024-01-01\n2,beta,13.7,2024-01-02\n3,gamma,99.1,2024-01-03\n".repeat(100).into_bytes()),
            ("JSON data", r#"{"users":[{"id":1,"name":"Alice","email":"alice@example.com"},{"id":2,"name":"Bob","email":"bob@example.com"}]}"#.repeat(50).into_bytes()),
            ("Binary random (5KB)", (0..5000).map(|i| ((i * 7 + 13) % 256) as u8).collect()),
            ("Repetitive binary (10KB)", vec![0xAA, 0xBB, 0xCC, 0xDD].into_iter().cycle().take(10000).collect()),
            ("SQL statements", "INSERT INTO users (id, name, email) VALUES (1, 'Alice', 'alice@test.com');\nSELECT * FROM users WHERE id > 0 ORDER BY name;\nUPDATE users SET email = 'new@test.com' WHERE id = 1;\n".repeat(50).into_bytes()),
            ("XML/HTML", "<html><head><title>Test</title></head><body><div class=\"container\"><p>Hello World</p></div></body></html>\n".repeat(80).into_bytes()),
        ];

        let total = test_cases.len();
        for (i, (name, data)) in test_cases.iter().enumerate() {
            let pct = ((i as f64 / total as f64) * 80.0) as u32 + 10;
            cb(&format!("Benchmarking: {} ({} bytes)...", name, data.len()), pct);

            let config = helix_core::pipeline::PipelineConfig::default();
            let mut pipeline = helix_core::pipeline::HelixPipeline::new(config);

            // Encode
            let encode_start = std::time::Instant::now();
            let enc = pipeline.encode(data, &format!("{}.dat", name), None);
            let encode_time = encode_start.elapsed().as_secs_f64();

            // Decode (no chaos)
            let decode_start = std::time::Instant::now();
            let dec = pipeline.decode(None);
            let decode_time = decode_start.elapsed().as_secs_f64();

            let (decode_ok, data_match) = match dec {
                Ok(ref d) => (true, d.data_match),
                Err(_) => (false, false),
            };

            let throughput_encode = if encode_time > 0.0 {
                data.len() as f64 / (1024.0 * 1024.0) / encode_time
            } else { 0.0 };

            let throughput_decode = if decode_time > 0.0 {
                data.len() as f64 / (1024.0 * 1024.0) / decode_time
            } else { 0.0 };

            results.push(serde_json::json!({
                "name": name,
                "input_size": data.len(),
                "compressed_size": enc.post_compress_size,
                "dna_bases": enc.transcode.sequence_length,
                "num_oligos": enc.num_oligos,
                "compression_ratio": enc.compression_stats.as_ref().map(|s| s.compression_ratio).unwrap_or(1.0),
                "space_saving_pct": enc.compression_stats.as_ref().map(|s| s.space_saving_percent).unwrap_or(0.0),
                "encode_time_ms": (encode_time * 1000.0 * 100.0).round() / 100.0,
                "decode_time_ms": (decode_time * 1000.0 * 100.0).round() / 100.0,
                "throughput_encode_mbps": (throughput_encode * 100.0).round() / 100.0,
                "throughput_decode_mbps": (throughput_decode * 100.0).round() / 100.0,
                "decode_ok": decode_ok,
                "data_match": data_match,
                "gc_content": enc.transcode.gc_content,
                "synthesis_readiness": enc.constraint_report.as_ref().map(|r| r.synthesis_readiness_score).unwrap_or(0.0),
                "cost_per_mb": enc.cost_estimate.as_ref().map(|c| c.cost_per_mb_stored).unwrap_or(0.0),
            }));
        }

        cb("Benchmark complete", 100);
        let all_pass = results.iter().all(|r| r["data_match"].as_bool().unwrap_or(false));

        let response = serde_json::json!({
            "success": true,
            "all_pass": all_pass,
            "num_tests": results.len(),
            "results": results,
        });

        update_task(&state_clone, &tid, |t| {
            t.status = "done".to_string();
            t.percent = 100;
            t.phase = "Complete".to_string();
            t.result = Some(response);
        });
    });

    HttpResponse::Ok().json(serde_json::json!({"task_id": task_id}))
}

// ========== File Type Detection ==========

fn detect_file_type_info(data: &[u8], filename: &str) -> serde_json::Value {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();

    let mime = match ext.as_str() {
        "txt" | "text" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "sql" => "application/sql",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "js" => "text/javascript",
        "css" => "text/css",
        "md" => "text/markdown",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "tar" => "application/x-tar",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        _ => {
            // Detect from content
            if data.len() >= 4 {
                if data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF { "image/jpeg" }
                else if &data[..4] == &[0x89, 0x50, 0x4E, 0x47] { "image/png" }
                else if data.starts_with(b"%PDF") { "application/pdf" }
                else if &data[..4] == &[0x50, 0x4B, 0x03, 0x04] { "application/zip" }
                else if &data[..2] == &[0x1F, 0x8B] { "application/gzip" }
                else {
                    let sample = &data[..data.len().min(512)];
                    let text_pct = sample.iter().filter(|&&b| b.is_ascii_graphic() || b.is_ascii_whitespace()).count() as f64 / sample.len() as f64;
                    if text_pct > 0.85 { "text/plain" } else { "application/octet-stream" }
                }
            } else { "application/octet-stream" }
        }
    };

    let category = if mime.starts_with("text/") || mime.contains("json") || mime.contains("xml") || mime.contains("sql") {
        "text"
    } else if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else {
        "binary"
    };

    let icon = match category {
        "text" => "📄",
        "image" => "🖼️",
        "video" => "🎬",
        "audio" => "🎵",
        _ => "📦",
    };

    serde_json::json!({
        "mime": mime,
        "extension": ext,
        "category": category,
        "icon": icon,
        "size": data.len(),
        "size_human": format_size(data.len()),
    })
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

async fn api_download_recovered(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);
    let pipeline = session.pipeline.lock().expect("lock poisoned");
    if let Some(ref dec) = pipeline.last_decode {
        if let Some(ref data) = dec.recovered_data {
            let fname = pipeline
                .last_encode
                .as_ref()
                .map(|e| e.output.filename.clone())
                .unwrap_or_else(|| "recovered".into());
            return HttpResponse::Ok()
                .insert_header((
                    "Content-Disposition",
                    format!("attachment; filename=\"recovered_{}\"", sanitize_filename(&fname)),
                ))
                .content_type(get_mime_type(&fname))
                .body(data.clone());
        }
    }
    HttpResponse::BadRequest().json(serde_json::json!({"error": "No recovered data."}))
}

async fn api_download_fasta(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);
    let pipeline = session.pipeline.lock().expect("lock poisoned");
    if let Some(ref enc) = pipeline.last_encode {
        // Use full FASTA content (not truncated JSON preview)
        return HttpResponse::Ok()
            .insert_header((
                "Content-Disposition",
                "attachment; filename=\"helix_core_output.fasta\"",
            ))
            .content_type("text/plain")
            .body(enc.full_fasta_content.clone());
    }
    HttpResponse::BadRequest().json(serde_json::json!({"error": "No FASTA data."}))
}

async fn api_download_original(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);
    let data = session.original_data.lock().expect("lock poisoned").clone();
    let fname = session
        .original_filename
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "original".into());
    match data {
        Some(d) => HttpResponse::Ok()
            .insert_header((
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", sanitize_filename(&fname)),
            ))
            .content_type(get_mime_type(&fname))
            .body(d),
        None => HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "No original data."})),
    }
}

async fn api_config_get(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);
    let pipeline = session.pipeline.lock().expect("lock poisoned");
    HttpResponse::Ok().json(pipeline.get_config_json())
}

async fn api_config_post(
    req: HttpRequest,
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> HttpResponse {
    let owner_key = client_key(&req);
    let session = get_or_create_session(state.get_ref(), &owner_key);
    let mut pipeline = session.pipeline.lock().expect("lock poisoned");
    pipeline.update_config(&body);
    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "config": pipeline.get_config_json(),
    }))
}

// ========== Multipart parser ==========

async fn parse_multipart(
    mut payload: Multipart,
) -> Result<(Vec<u8>, String, f64), String> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut filename = "unknown".to_string();
    let mut redundancy = 1.5f64;

    while let Some(mut field) = payload
        .try_next()
        .await
        .map_err(|e| format!("Multipart parse error: {}", e))?
    {
        let cd = field.content_disposition();
        let name = cd.map(|c| c.get_name().unwrap_or("").to_string()).unwrap_or_default();
        let field_filename = cd
            .and_then(|c| c.get_filename().map(|s| s.to_string()));

        let mut data = Vec::new();
        while let Some(chunk) = field
            .try_next()
            .await
            .map_err(|e| format!("Multipart field error: {}", e))?
        {
            if data.len() + chunk.len() > MAX_UPLOAD_BYTES {
                return Err("Payload too large".to_string());
            }
            data.extend_from_slice(&chunk);
        }

        match name.as_str() {
            "file" => {
                filename = field_filename.unwrap_or_else(|| "unknown".to_string());
                file_data = Some(data);
            }
            "redundancy" => {
                if let Ok(s) = String::from_utf8(data) {
                    redundancy = s.trim().parse().unwrap_or(2.0);
                }
            }
            "text" => {
                file_data = Some(data);
                filename = "text_input.txt".to_string();
            }
            _ => {}
        }
    }

    file_data
        .map(|d| (d, filename, redundancy))
        .ok_or_else(|| "No file provided.".to_string())
}

// ========== Health Check ==========

async fn api_health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "healthy",
        "version": "5.0.0",
        "service": "helix-core",
        "features": [
            "hypercompress_parallel", "interleaved_reed_solomon", "fountain_robust_soliton",
            "oligo_builder", "dna_constraints", "cost_estimator", "sse_progress"
        ],
    }))
}

// ========== Error Profile Presets ==========

async fn api_error_profiles() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "profiles": {
            "illumina": {
                "name": "Illumina NGS",
                "description": "High accuracy, short reads (150-300bp)",
                "loss_rate": 0.05,
                "deletion_rate": 0.001,
                "substitution_rate": 0.005,
                "insertion_rate": 0.001,
            },
            "nanopore": {
                "name": "Oxford Nanopore",
                "description": "Long reads, higher error rates",
                "loss_rate": 0.10,
                "deletion_rate": 0.04,
                "substitution_rate": 0.05,
                "insertion_rate": 0.03,
            },
            "pacbio_hifi": {
                "name": "PacBio HiFi",
                "description": "Long reads, high accuracy CCS",
                "loss_rate": 0.08,
                "deletion_rate": 0.002,
                "substitution_rate": 0.003,
                "insertion_rate": 0.001,
            },
            "aging_1000yr": {
                "name": "1000-Year Aging",
                "description": "Simulates DNA degradation over 1000 years",
                "loss_rate": 0.30,
                "deletion_rate": 0.10,
                "substitution_rate": 0.15,
                "insertion_rate": 0.05,
            },
            "catastrophic": {
                "name": "Catastrophic Failure",
                "description": "Extreme damage stress test",
                "loss_rate": 0.60,
                "deletion_rate": 0.20,
                "substitution_rate": 0.25,
                "insertion_rate": 0.10,
            },
        }
    }))
}

// ========== Route: Custom Benchmark ==========

#[derive(Deserialize)]
struct CustomBenchmarkInput {
    text: Option<String>,
    redundancy: Option<Vec<f64>>,
    block_sizes: Option<Vec<usize>>,
    chaos_profiles: Option<Vec<String>>,
    iterations: Option<usize>,
}

async fn api_benchmark_custom(
    req: HttpRequest,
    payload: web::Payload,
    state: web::Data<AppState>,
) -> HttpResponse {
    let sid = client_key(&req);
    info!("api_benchmark_custom started for session {sid}");

    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let (data, params) = if ct.contains("multipart") {
        match parse_multipart(Multipart::new(req.headers(), payload)).await {
            Ok((file_data, _filename, _redundancy)) => {
                // For multipart, use defaults for benchmark params
                (file_data, CustomBenchmarkInput {
                    text: None,
                    redundancy: None,
                    block_sizes: None,
                    chaos_profiles: None,
                    iterations: None,
                })
            }
            Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({"error": e})),
        }
    } else {
        let mut body = Vec::<u8>::new();
        let mut stream = payload;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => {
                    if body.len() + c.len() > MAX_UPLOAD_BYTES {
                        return HttpResponse::PayloadTooLarge()
                            .json(serde_json::json!({"error": "Payload too large"}));
                    }
                    body.extend_from_slice(&c);
                }
                Err(e) => {
                    return HttpResponse::BadRequest()
                        .json(serde_json::json!({"error": e.to_string()}))
                }
            }
        }
        let input: CustomBenchmarkInput = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return HttpResponse::BadRequest()
                    .json(serde_json::json!({"error": e.to_string()}))
            }
        };
        let text_data = input.text.clone().unwrap_or_default().into_bytes();
        if text_data.is_empty() {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "No data provided. Supply 'text' field or use multipart file upload."}));
        }
        (text_data, input)
    };

    if data.is_empty() {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "Empty data!"}));
    }

    let redundancies = params.redundancy.unwrap_or_else(|| vec![1.5, 2.0, 2.5, 3.0]);
    let block_sizes = params.block_sizes.unwrap_or_else(|| vec![32, 64, 128]);
    let chaos_profile_names = params.chaos_profiles.unwrap_or_else(|| {
        vec!["illumina".into(), "nanopore".into(), "pacbio_hifi".into(), "aging_1000yr".into()]
    });
    let iterations = params.iterations.unwrap_or(1).min(10).max(1);

    // Validate parameters
    for &r in &redundancies {
        if !r.is_finite() || r <= 0.0 || r > 20.0 {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": format!("Invalid redundancy value: {}", r)}));
        }
    }
    for &bs in &block_sizes {
        if bs == 0 || bs > 1024 {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": format!("Invalid block size: {}", bs)}));
        }
    }

    // Build chaos profiles lookup
    let chaos_profiles: Vec<(String, f64, Option<f64>, Option<f64>, Option<f64>)> = chaos_profile_names.iter().map(|name| {
        match name.as_str() {
            "illumina" => (name.clone(), 0.05, Some(0.001), Some(0.005), Some(0.001)),
            "nanopore" => (name.clone(), 0.10, Some(0.04), Some(0.05), Some(0.03)),
            "pacbio_hifi" => (name.clone(), 0.08, Some(0.002), Some(0.003), Some(0.001)),
            "aging_1000yr" => (name.clone(), 0.30, Some(0.10), Some(0.15), Some(0.05)),
            "catastrophic" => (name.clone(), 0.60, Some(0.20), Some(0.25), Some(0.10)),
            "none" => (name.clone(), 0.0, Some(0.0), Some(0.0), Some(0.0)),
            _ => (name.clone(), 0.10, Some(0.01), Some(0.01), Some(0.01)),
        }
    }).collect();

    let total_combos = redundancies.len() * block_sizes.len() * chaos_profiles.len() * iterations;
    if total_combos > 500 {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": format!("Too many combinations ({}). Max 500.", total_combos)}));
    }

    let task_id = new_task(state.get_ref(), &sid);
    let state_clone = state.into_inner().clone();
    let tid = task_id.clone();

    tokio::task::spawn_blocking(move || {
        let cb_state = state_clone.clone();
        let cb_tid = tid.clone();

        let mut results = Vec::new();
        let mut combo_idx = 0usize;

        for &redundancy in &redundancies {
            for &block_size in &block_sizes {
                for (profile_name, loss, del, sub, ins) in &chaos_profiles {
                    let mut iter_results = Vec::new();

                    for iter_i in 0..iterations {
                        combo_idx += 1;
                        let pct = ((combo_idx as f64 / total_combos as f64) * 90.0) as u32 + 5;
                        let phase_msg = format!(
                            "[{}/{}] R={:.1}x BS={} {} iter {}/{}",
                            combo_idx, total_combos, redundancy, block_size, profile_name, iter_i + 1, iterations
                        );
                        update_task(&cb_state, &cb_tid, |t| {
                            t.phase = phase_msg;
                            t.percent = pct;
                        });

                        let mut config = helix_core::pipeline::PipelineConfig::default();
                        config.redundancy = redundancy;
                        config.block_size = block_size;
                        let mut pipeline = helix_core::pipeline::HelixPipeline::new(config);

                        // Encode
                        let encode_start = std::time::Instant::now();
                        let _enc = pipeline.encode(&data, "benchmark_input.dat", None);
                        let encode_time = encode_start.elapsed().as_secs_f64();

                        // Apply chaos
                        let chaos_start = std::time::Instant::now();
                        let chaos_result = pipeline.apply_chaos(*loss, *del, *sub, *ins, None);
                        let chaos_time = chaos_start.elapsed().as_secs_f64();

                        // Decode
                        let decode_start = std::time::Instant::now();
                        let dec = pipeline.decode(None);
                        let decode_time = decode_start.elapsed().as_secs_f64();

                        let (decode_ok, data_match) = match &dec {
                            Ok(d) => (true, d.data_match),
                            Err(_) => (false, false),
                        };

                        let survival_rate = chaos_result.as_ref().ok()
                            .map(|c| c.droplet_survival_rate).unwrap_or(1.0);

                        iter_results.push(serde_json::json!({
                            "iteration": iter_i + 1,
                            "encode_time_ms": (encode_time * 1000.0 * 100.0).round() / 100.0,
                            "chaos_time_ms": (chaos_time * 1000.0 * 100.0).round() / 100.0,
                            "decode_time_ms": (decode_time * 1000.0 * 100.0).round() / 100.0,
                            "decode_ok": decode_ok,
                            "data_match": data_match,
                            "survival_rate": survival_rate,
                        }));
                    }

                    // Compute averages across iterations
                    let avg_encode = iter_results.iter()
                        .filter_map(|r| r["encode_time_ms"].as_f64())
                        .sum::<f64>() / iterations as f64;
                    let avg_chaos = iter_results.iter()
                        .filter_map(|r| r["chaos_time_ms"].as_f64())
                        .sum::<f64>() / iterations as f64;
                    let avg_decode = iter_results.iter()
                        .filter_map(|r| r["decode_time_ms"].as_f64())
                        .sum::<f64>() / iterations as f64;
                    let all_match = iter_results.iter()
                        .all(|r| r["data_match"].as_bool().unwrap_or(false));
                    let recovery_rate = iter_results.iter()
                        .filter(|r| r["data_match"].as_bool().unwrap_or(false))
                        .count() as f64 / iterations as f64;

                    // Run one more encode with no chaos for metrics
                    let mut config = helix_core::pipeline::PipelineConfig::default();
                    config.redundancy = redundancy;
                    config.block_size = block_size;
                    let mut pipeline = helix_core::pipeline::HelixPipeline::new(config);
                    let enc = pipeline.encode(&data, "benchmark_input.dat", None);

                    let throughput_encode = if avg_encode > 0.0 {
                        data.len() as f64 / (1024.0 * 1024.0) / (avg_encode / 1000.0)
                    } else { 0.0 };
                    let throughput_decode = if avg_decode > 0.0 {
                        data.len() as f64 / (1024.0 * 1024.0) / (avg_decode / 1000.0)
                    } else { 0.0 };

                    results.push(serde_json::json!({
                        "redundancy": redundancy,
                        "block_size": block_size,
                        "chaos_profile": profile_name,
                        "iterations": iterations,
                        "input_size": data.len(),
                        "compressed_size": enc.post_compress_size,
                        "compression_ratio": enc.compression_stats.as_ref().map(|s| s.compression_ratio).unwrap_or(1.0),
                        "space_saving_pct": enc.compression_stats.as_ref().map(|s| s.space_saving_percent).unwrap_or(0.0),
                        "dna_bases": enc.transcode.sequence_length,
                        "num_oligos": enc.num_oligos,
                        "gc_content": enc.transcode.gc_content,
                        "homopolymer_safe": enc.transcode.homopolymer_safe,
                        "synthesis_readiness": enc.constraint_report.as_ref().map(|r| r.synthesis_readiness_score).unwrap_or(0.0),
                        "cost_per_mb": enc.cost_estimate.as_ref().map(|c| c.cost_per_mb_stored).unwrap_or(0.0),
                        "total_cost_usd": enc.cost_estimate.as_ref().map(|c| c.total_cost_usd).unwrap_or(0.0),
                        "avg_encode_time_ms": (avg_encode * 100.0).round() / 100.0,
                        "avg_chaos_time_ms": (avg_chaos * 100.0).round() / 100.0,
                        "avg_decode_time_ms": (avg_decode * 100.0).round() / 100.0,
                        "total_time_ms": ((avg_encode + avg_chaos + avg_decode) * 100.0).round() / 100.0,
                        "throughput_encode_mbps": (throughput_encode * 100.0).round() / 100.0,
                        "throughput_decode_mbps": (throughput_decode * 100.0).round() / 100.0,
                        "all_iterations_match": all_match,
                        "recovery_rate": recovery_rate,
                        "per_iteration": iter_results,
                    }));
                }
            }
        }

        // Compute summary statistics
        let all_pass = results.iter().all(|r| r["all_iterations_match"].as_bool().unwrap_or(false));
        let best_idx = results.iter().enumerate()
            .filter(|(_, r)| r["all_iterations_match"].as_bool().unwrap_or(false))
            .min_by(|(_, a), (_, b)| {
                let cost_a = a["total_time_ms"].as_f64().unwrap_or(f64::MAX);
                let cost_b = b["total_time_ms"].as_f64().unwrap_or(f64::MAX);
                cost_a.partial_cmp(&cost_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);
        let worst_idx = results.iter().enumerate()
            .max_by(|(_, a), (_, b)| {
                let cost_a = a["total_time_ms"].as_f64().unwrap_or(0.0);
                let cost_b = b["total_time_ms"].as_f64().unwrap_or(0.0);
                cost_a.partial_cmp(&cost_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);

        let avg_recovery: f64 = if results.is_empty() { 0.0 } else {
            results.iter().filter_map(|r| r["recovery_rate"].as_f64()).sum::<f64>() / results.len() as f64
        };
        let avg_encode_time: f64 = if results.is_empty() { 0.0 } else {
            results.iter().filter_map(|r| r["avg_encode_time_ms"].as_f64()).sum::<f64>() / results.len() as f64
        };
        let avg_decode_time: f64 = if results.is_empty() { 0.0 } else {
            results.iter().filter_map(|r| r["avg_decode_time_ms"].as_f64()).sum::<f64>() / results.len() as f64
        };

        update_task(&cb_state, &cb_tid, |t| {
            t.phase = "Benchmark complete".to_string();
            t.percent = 100;
        });

        let response = serde_json::json!({
            "success": true,
            "all_pass": all_pass,
            "num_configurations": results.len(),
            "input_size": data.len(),
            "parameters": {
                "redundancies": redundancies,
                "block_sizes": block_sizes,
                "chaos_profiles": chaos_profile_names,
                "iterations": iterations,
            },
            "summary": {
                "best_config_index": best_idx,
                "worst_config_index": worst_idx,
                "avg_recovery_rate": (avg_recovery * 10000.0).round() / 10000.0,
                "avg_encode_time_ms": (avg_encode_time * 100.0).round() / 100.0,
                "avg_decode_time_ms": (avg_decode_time * 100.0).round() / 100.0,
            },
            "results": results,
        });

        update_task(&state_clone, &tid, |t| {
            t.status = "done".to_string();
            t.percent = 100;
            t.phase = "Complete".to_string();
            t.result = Some(response);
        });
    });

    HttpResponse::Ok().json(serde_json::json!({"task_id": task_id}))
}

// ========== Serve benchmark.html ==========

async fn serve_benchmark() -> HttpResponse {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("static")
        .join("benchmark.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(content),
        Err(_) => HttpResponse::NotFound().body("Benchmark page not found"),
    }
}

// ========== Serve index.html ==========

async fn serve_index() -> HttpResponse {
    // Serve from local static/ directory (Rust-native frontend)
    let index_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("static")
        .join("index.html");

    match tokio::fs::read_to_string(&index_path).await {
        Ok(content) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(content),
        Err(_) => HttpResponse::Ok()
            .content_type("text/html")
            .body(include_str!("fallback.html")),
    }
}

// ========== Main ==========

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let state = web::Data::new(AppState {
        tasks: RwLock::new(HashMap::new()),
        sessions: RwLock::new(HashMap::new()),
        sse_channels: RwLock::new(HashMap::new()),
    });

    // Static files directory (Rust-native frontend)
    let static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("static");

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5000);

    println!("============================================================");
    println!("  Project Helix-Core v5.0: DNA Data Storage OS — Rust Edition");
    println!("  Server at: http://localhost:{port}");
    println!("  Pipeline: HyperCompress → Interleaved-RS → Fountain → Transcode");
    println!("           → OligoBuilder → Constraints → FASTA → Cost");
    println!("  Progress: SSE (Server-Sent Events) — zero-polling");
    println!("  Threaded task execution with real-time progress");
    println!("============================================================");

    HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);

        App::new()
            .wrap(Logger::default())
            .wrap(cors)
            .app_data(state.clone())
            .app_data(web::PayloadConfig::new(500 * 1024 * 1024)) // 500MB payload limit
            .app_data(web::JsonConfig::default().limit(500 * 1024 * 1024)) // 500MB JSON limit
            .route("/", web::get().to(serve_index))
            .route("/api/health", web::get().to(api_health))
            .route("/api/progress/{task_id}", web::get().to(api_progress))
            .route("/api/events/{task_id}", web::get().to(api_events))
            .route("/api/encode", web::post().to(api_encode))
            .route("/api/chaos", web::post().to(api_chaos))
            .route("/api/decode", web::post().to(api_decode))
            .route("/api/decode_fasta", web::post().to(api_decode_fasta))
            .route("/api/benchmark", web::post().to(api_benchmark))
            .route("/api/benchmark_custom", web::post().to(api_benchmark_custom))
            .route("/benchmark", web::get().to(serve_benchmark))
            .route("/api/error_profiles", web::get().to(api_error_profiles))
            .route(
                "/api/download_recovered",
                web::get().to(api_download_recovered),
            )
            .route(
                "/api/download_fasta",
                web::get().to(api_download_fasta),
            )
            .route(
                "/api/download_original",
                web::get().to(api_download_original),
            )
            .route("/api/config", web::get().to(api_config_get))
            .route("/api/config", web::post().to(api_config_post))
            .service(fs::Files::new("/static", &static_dir))
    })
    .bind(format!("0.0.0.0:{port}"))?
    .workers(4)
    .run()
    .await
}
