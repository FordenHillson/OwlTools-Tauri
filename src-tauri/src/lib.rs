// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use tauri::tray::{MouseButton, MouseButtonState};
use std::fs;
use std::net::TcpStream;
use std::collections::{HashMap, HashSet, BTreeMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use serde_json::Value as JsonValue;
use tauri::Manager;
use tauri::Emitter;
use serde::{Serialize, Deserialize};
use serde_json::json;
use axum::{Router, routing::{get, post}, extract::State as AxumState, Json};
use axum::response::sse::{Sse, Event, KeepAlive};
use tower_http::cors::CorsLayer;
use std::sync::{Arc, Mutex};
use once_cell::sync::OnceCell;
use tokio::{process::Command as TokioCommand, io::{AsyncBufReadExt, BufReader}, time::{sleep, Duration}, sync::mpsc};
use tokio::io::AsyncWriteExt;
use std::process::Stdio;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use futures::StreamExt;
use std::convert::Infallible;
use walkdir::WalkDir;
use dirs_next::{home_dir, document_dir};
use chrono::Utc;
use std::time::UNIX_EPOCH;
use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::BTreeSet;
use sha2::{Digest, Sha256};
use rand::Rng;
use regex::Regex;

#[derive(Serialize, Clone)]
struct ScanLogPayload {
    level: String,
    message: String,
    current: Option<usize>,
    total: Option<usize>,
}

#[derive(Serialize, Clone)]
struct MetaEntry { guid: String, path: String }

#[derive(Serialize, Clone)]
struct FullDstZoneInfo {
    part_id: String,
    debris: Vec<MetaEntry>,
    colliders: Vec<String>,
}

#[derive(Serialize, Clone)]
struct FullDstScanResult {
    base_guid: String,
    base_path: String,
    v2_guid: String,
    v2_path: String,
    zones: Vec<FullDstZoneInfo>,
}

#[tauri::command]
async fn prefabdst_read_meta(xob_path: String) -> Result<MetaEntry, String> {
    let xob_abs = PathBuf::from(&xob_path);
    match read_xob_object_field_from_meta(&xob_abs) {
        Ok((g, p)) => Ok(MetaEntry { guid: g, path: p }),
        Err(_) => Ok(MetaEntry { guid: String::new(), path: xob_path }),
    }
}

fn parse_geometry_param_names(v2_meta_text: &str) -> Vec<String> {
    // Extract names from lines like:
    //  GeometryParam UTM_Base_Ruin_base {
    //  GeometryParam "UTM_House_..." {
    let re = Regex::new(r#"(?m)^\s*GeometryParam\s+(?:\"([^\"]+)\"|([^\"\s\{]+))\s*\{"#).unwrap();
    let mut out: Vec<String> = Vec::new();
    for cap in re.captures_iter(v2_meta_text) {
        let name = cap.get(1).or_else(|| cap.get(2)).map(|m| m.as_str()).unwrap_or("");
        let n = name.trim();
        if !n.is_empty() {
            out.push(n.to_string());
        }
    }
    // Preserve order but remove duplicates
    let mut seen: HashSet<String> = HashSet::new();
    let mut dedup: Vec<String> = Vec::new();
    for n in out {
        let key = n.to_lowercase();
        if seen.insert(key) {
            dedup.push(n);
        }
    }
    dedup
}

fn geometry_param_belongs_to_part(name: &str, part_id: &str) -> bool {
    let n = name.to_lowercase();
    let p = part_id.to_lowercase();
    if p.is_empty() { return false; }

    // Include exact part tags commonly used by Arma prefabs
    if n.contains(&format!("id-{}", p)) { return true; }
    if n.contains(&format!("fdst_id-{}", p)) { return true; }
    // VIS tags like: ..._VIS-A^B, ..._VIS-!A, ..._VIS-!A^!B
    if n.contains(&format!("vis-{}", p)) { return true; }
    if n.contains(&format!("vis-!{}", p)) { return true; }
    false
}

#[tauri::command]
async fn prefabdst_scan_full_dst(xob_path: String) -> Result<FullDstScanResult, String> {
    let xob_abs = PathBuf::from(&xob_path);
    if !xob_abs.is_file() { return Err("Invalid xob path".into()); }

    let (base_guid, base_path) = read_xob_object_field_from_meta(&xob_abs)?;

    let stem = xob_abs
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Invalid xob file name".to_string())?;
    let parent = xob_abs.parent().ok_or_else(|| "Invalid xob directory".to_string())?;

    // sibling v2 model
    let v2_xob = parent.join(format!("{}_V2_dst.xob", stem));
    if !v2_xob.is_file() {
        return Err("V2 dst file not found next to base xob".into());
    }
    let (v2_guid, v2_path) = read_xob_object_field_from_meta(&v2_xob)?;

    // Parse GeometryParams from v2 meta
    let v2_meta_path = PathBuf::from(format!("{}.meta", v2_xob.to_string_lossy()));
    let v2_meta_text = fs::read_to_string(&v2_meta_path)
        .map_err(|e| format!("Failed to read v2 .xob.meta: {}", e))?;
    let mut geom_params = parse_geometry_param_names(&v2_meta_text);

    let base_meta_path = PathBuf::from(format!("{}.meta", xob_abs.to_string_lossy()));
    if let Ok(base_meta_text) = fs::read_to_string(&base_meta_path) {
        let base_params = parse_geometry_param_names(&base_meta_text)
            .into_iter()
            .filter(|n| n.to_lowercase().contains("fdst_"));
        let mut seen: HashSet<String> = geom_params.iter().map(|s| s.to_lowercase()).collect();
        for p in base_params {
            let key = p.to_lowercase();
            if seen.insert(key) {
                geom_params.push(p);
            }
        }
    }

    // Scan debris from Dst folder
    let mut debris_by_part: BTreeMap<String, Vec<(i32, MetaEntry)>> = BTreeMap::new();
    if let Some(dst_dir) = find_dst_directory(&xob_abs) {
        let rx = Regex::new(&format!(r"(?i)^{}_V2_dst_ID-(?P<p>[A-Z])_dbr_(?P<d>\d+)\.xob$", regex::escape(stem))).unwrap();
        if let Ok(rd) = fs::read_dir(&dst_dir) {
            for ent in rd.flatten() {
                let p = ent.path();
                if !p.is_file() { continue; }
                let name = match p.file_name().and_then(|s| s.to_str()) { Some(s) => s, None => continue };
                if let Some(c) = rx.captures(name) {
                    let pid = c.name("p").map(|m| m.as_str()).unwrap_or("").to_string();
                    let dn = c.name("d").map(|m| m.as_str()).unwrap_or("0").parse::<i32>().unwrap_or(0);
                    let entry = match read_xob_object_field_from_meta(&p) {
                        Ok((g, r)) => MetaEntry { guid: g, path: r },
                        Err(_) => MetaEntry { guid: String::new(), path: rel_from_known_roots(&p) },
                    };
                    debris_by_part.entry(pid).or_default().push((dn, entry));
                }
            }
        }
    }

    // Determine which part IDs exist.
    // Previously we built zones only from debris. That breaks collider replacement when a model has
    // no dbr files. Include parts inferred from GeometryParam names as well.
    let mut part_ids: BTreeMap<String, ()> = BTreeMap::new();
    for pid in debris_by_part.keys() {
        part_ids.insert(pid.clone(), ());
    }
    {
        let re_part = Regex::new(r"(?i)(?:id-|fdst_id-|vis-|vis-!)(?P<p>[a-z])").unwrap();
        for n in &geom_params {
            if let Some(c) = re_part.captures(n) {
                let pid = c.name("p").map(|m| m.as_str()).unwrap_or("").to_uppercase();
                if pid.len() == 1 {
                    part_ids.insert(pid, ());
                }
            }
        }
    }

    // Sort debris by dbr number and build zones list.
    let mut zones: Vec<FullDstZoneInfo> = Vec::new();
    for (pid, _) in part_ids {
        let mut items = debris_by_part.remove(&pid).unwrap_or_default();
        items.sort_by_key(|(n, _)| *n);
        let debris: Vec<MetaEntry> = items.into_iter().map(|(_, e)| e).collect();
        let colliders: Vec<String> = geom_params
            .iter()
            .filter(|n| geometry_param_belongs_to_part(n, &pid))
            .cloned()
            .collect();
        zones.push(FullDstZoneInfo { part_id: pid, debris, colliders });
    }

    Ok(FullDstScanResult { base_guid, base_path, v2_guid, v2_path, zones })
}

fn extract_template_body(all_text: &str) -> String {
    // Prefer content between '--- TEMPLATE ---' and '--- END TEMPLATE ---' (case-insensitive)
    let nl = detect_newline(all_text);
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    let mut offset = 0usize;
    for ln in all_text.lines() {
        let low = ln.trim().to_lowercase();
        if start.is_none() && low.contains("--- template ---") {
            start = Some(offset + ln.len() + nl.len());
        } else if start.is_some() && low.contains("--- end template ---") {
            end = Some(offset);
            break;
        }
        offset += ln.len() + nl.len();
    }
    if let (Some(s), Some(e)) = (start, end) {
        return all_text[s..e].to_string();
    }
    // Fallback: strip header only
    strip_preset_header(all_text)
}

#[derive(Serialize, Deserialize, Clone)]
struct ScrDebrisItem { guid: String, path: String }
#[derive(Serialize, Deserialize, Clone)]
struct ScrPhaseItem { pid: String, model_guid: String, model_path: String, debris: Vec<ScrDebrisItem> }
#[derive(Serialize, Deserialize, Clone)]
struct ScrDstScanResult { base_guid: String, base_path: String, phases: Vec<ScrPhaseItem> }

#[tauri::command]
async fn prefabdst_scan_dst(xob_path: String) -> Result<ScrDstScanResult, String> {
    let xob_abs = PathBuf::from(&xob_path);
    if !xob_abs.is_file() { return Err("Invalid xob path".into()); }
    let (base_guid, base_path) = read_xob_object_field_from_meta(&xob_abs)?;
    let (phases_vec, debris_map) = scan_dst_for_phases_debris(&xob_abs);
    let mut phases: Vec<ScrPhaseItem> = Vec::new();
    for (pid, (g, p)) in phases_vec {
        let debris_items: Vec<ScrDebrisItem> = debris_map
            .get(&pid)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(dg, dp)| ScrDebrisItem { guid: dg, path: dp })
            .collect();
        phases.push(ScrPhaseItem { pid, model_guid: g, model_path: p, debris: debris_items });
    }
    Ok(ScrDstScanResult { base_guid, base_path, phases })
}

#[derive(Default)]
struct PresetHeader {
    id: String,
    title: String,
    project: String,
    generator: String,
    description: String,
}

fn parse_preset_header(all_text: &str) -> PresetHeader {
    let mut h = PresetHeader::default();
    for ln in all_text.lines() {
        let line = ln.trim();
        if line.is_empty() { break; }
        if let Some(col) = line.find(':') {
            let k = line[..col].trim().to_lowercase();
            let v = line[col+1..].trim();
            match k.as_str() {
                "id" => h.id = v.to_string(),
                "title" => h.title = v.to_string(),
                "project" => h.project = v.to_string(),
                "generator" => h.generator = v.to_string(),
                "description" => h.description = v.to_string(),
                _ => {}
            }
        }
    }
    h
}

fn find_dst_directory(xob_abs: &Path) -> Option<PathBuf> {
    let parent = xob_abs.parent()?;
    let exact = parent.join("dst");
    if exact.is_dir() { return Some(exact); }
    // case-insensitive search among siblings
    if let Ok(rd) = fs::read_dir(parent) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name.eq_ignore_ascii_case("dst") {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

fn scan_dst_for_phases_debris(base_xob_abs: &Path) -> (Vec<(String, (String, String))>, std::collections::HashMap<String, Vec<(String, String)>>) {
    let mut phases: Vec<(String, (String, String))> = Vec::new();
    let mut debris: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();
    let Some(dst_dir) = find_dst_directory(base_xob_abs) else { return (phases, debris); };

    let stem = base_xob_abs.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let base_cf = stem;
    let rx_phase = Regex::new(&format!(r"(?i){}{}_dst_(?P<p>\d+)\.xob$", regex::escape(""), regex::escape(&base_cf))).unwrap();
    let rx_debris = Regex::new(&format!(r"(?i){}{}_dst_(?P<p>\d+)_dbr_(?P<d>\d+)\.xob$", regex::escape(""), regex::escape(&base_cf))).unwrap();

    if let Ok(rd) = fs::read_dir(&dst_dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_file() { continue; }
            let name = match p.file_name().and_then(|s| s.to_str()) { Some(s)=>s, None=>continue };
            if let Some(c) = rx_phase.captures(name) {
                let pid_raw = c.name("p").map(|m| m.as_str()).unwrap_or("");
                let pid = format!("{:0>2}", pid_raw);
                match read_xob_object_field_from_meta(&p) {
                    Ok((g, r)) => phases.push((pid, (g, r))),
                    Err(_) => phases.push((pid, (String::new(), String::new()))),
                }
                continue;
            }
            if let Some(c) = rx_debris.captures(name) {
                let pid_raw = c.name("p").map(|m| m.as_str()).unwrap_or("");
                let pid = format!("{:0>2}", pid_raw);
                match read_xob_object_field_from_meta(&p) {
                    Ok((g, r)) => debris.entry(pid).or_default().push((g, r)),
                    Err(_) => debris.entry(pid).or_default().push((String::new(), String::new())),
                }
            }
        }
    }
    // Ensure every PID that has debris also has a phase entry (even if empty)
    {
        use std::collections::HashSet;
        let have: HashSet<String> = phases.iter().map(|(p, _)| p.clone()).collect();
        for pid in debris.keys() {
            if !have.contains(pid) {
                phases.push((pid.clone(), (String::new(), String::new())));
            }
        }
    }
    // sort phases by pid numeric
    phases.sort_by_key(|(pid, _)| pid.parse::<i32>().unwrap_or(9999));
    // sort debris by trailing _dbr_YY number inferred from path, fallback by name
    for (_pid, arr) in debris.iter_mut() {
        arr.sort_by_key(|(_g, path)| {
            let name = Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or("");
            let re = Regex::new(r"(?i)_dbr_(\d+)\.xob$").unwrap();
            if let Some(c) = re.captures(name) { c[1].parse::<i32>().unwrap_or(9999) } else { 9999 }
        });
    }
    (phases, debris)
}

fn render_scr_destructible_template(
    preset_text: &str,
    base_guid: &str,
    base_res: &str,
    base_xob_abs: &Path,
    override_data: Option<&ScrDstScanResult>,
) -> String {
    let nl = detect_newline(preset_text);
    let mut body = extract_template_body(preset_text);

    // Generate IDs
    let entity_id = gen_guid16();
    let mesh_id = gen_guid16();
    let id1 = gen_guid16();
    let id2 = gen_guid16();
    let id3 = gen_guid16();
    let id4 = gen_guid16();
    let id5 = gen_guid16();
    let id6 = gen_guid16();

    // Base object line (prefer override base if provided)
    let (use_guid, use_res) = if let Some(ov) = override_data {
        if !ov.base_guid.is_empty() || !ov.base_path.is_empty() {
            (ov.base_guid.clone(), ov.base_path.clone())
        } else { (base_guid.to_string(), base_res.to_string()) }
    } else {
        (base_guid.to_string(), base_res.to_string())
    };
    let base_object_line = if !use_guid.is_empty() && !use_res.is_empty() {
        format!("   Object \"{{{}}}{}\"", use_guid, use_res)
    } else if !base_guid.is_empty() && !base_res.is_empty() {
        format!("   Object \"{{{}}}{}\"", base_guid, base_res)
    } else {
        "   Object \"\"".to_string()
    };

    // Scan dst for phases and debris, unless override provided from UI
    let (phases, debris_map) = if let Some(ov) = override_data {
        let mut phases: Vec<(String, (String, String))> = Vec::new();
        let mut debris: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();
        for ph in &ov.phases {
            phases.push((ph.pid.clone(), (ph.model_guid.clone(), ph.model_path.clone())));
            let mut arr: Vec<(String, String)> = Vec::new();
            for d in &ph.debris { arr.push((d.guid.clone(), d.path.clone())); }
            debris.insert(ph.pid.clone(), arr);
        }
        (phases, debris)
    } else {
        scan_dst_for_phases_debris(base_xob_abs)
    };
    let first_phase = phases.first().map(|(_pid,(g,r))| (g.clone(), r.clone()));
    let last_phase = phases.last().map(|(_pid,(g,r))| (g.clone(), r.clone()));
    let first_pid = phases.first().map(|(pid, _)| pid.clone());
    let last_pid = phases.last().map(|(pid, _)| pid.clone());

    let first_phase_model = first_phase.map(|(g,r)| format!("\"{{{}}}{}\"", g, r)).unwrap_or_else(|| "\"\"".to_string());
    let last_phase_model = last_phase.map(|(g,r)| format!("\"{{{}}}{}\"", g, r)).unwrap_or_else(|| "\"\"".to_string());

    let join_debris = |pid_opt: Option<String>| -> String {
        if let Some(pid) = pid_opt {
            if let Some(arr) = debris_map.get(&pid) {
                let parts: Vec<String> = arr.iter().map(|(g,r)| format!("\"{{{}}}{}\"", g, r)).collect();
                return parts.join(" ");
            }
        }
        String::new()
    };
    let first_phase_debris = join_debris(first_pid);
    let last_phase_debris = join_debris(last_pid);

    // Replace placeholders
    let replaces = [
        ("{{ENTITY_ID}}", &entity_id),
        ("{{MESH_ID_BRACED}}", &format!("{{{}}}", mesh_id)),
        ("{{ID1}}", &id1),
        ("{{ID2}}", &id2),
        ("{{ID3}}", &id3),
        ("{{ID4}}", &id4),
        ("{{ID5}}", &id5),
        ("{{ID6}}", &id6),
        ("{{BASE_OBJECT_LINE}}", &base_object_line),
        ("{{FIRST_PHASE_MODEL}}", &first_phase_model),
        ("{{LAST_PHASE_MODEL}}", &last_phase_model),
        ("{{FIRST_PHASE_DEBRIS}}", &first_phase_debris),
        ("{{LAST_PHASE_DEBRIS}}", &last_phase_debris),
    ];
    for (k, v) in replaces {
        body = body.replace(k, v);
    }
    // Ensure newline style remains consistent
    if nl == "\r\n" { body = body.replace("\n", "\r\n"); }
    body
}

#[derive(Serialize, Clone)]
struct PrefabDstLogPayload {
    level: String,
    message: String,
    current: Option<usize>,
    total: Option<usize>,
}

#[tauri::command]
fn get_display_version(app: tauri::AppHandle) -> String {
    let version = app.package_info().version.to_string();
    let display_version = option_env!("OWLTOOLS_BUILD_TAG").unwrap_or(version.as_str());
    display_version.to_string()
}

fn gen_hex16() -> String {
    static CTR: AtomicU64 = AtomicU64::new(1);
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    format!("{:016X}", now ^ (c.wrapping_mul(0x9E3779B97F4A7C15)))
}

fn gen_guid16() -> String {
    // Match Python behavior: random 64-bit -> 16 hex
    let v: u64 = rand::thread_rng().gen();
    format!("{:016X}", v)
}

fn build_et_meta_text(name_value: &str) -> String {
    let mut out = String::new();
    out.push_str("MetaFileClass {\n");
    out.push_str(&format!(" Name \"{}\"\n", name_value));
    out.push_str(" Configurations {\n");
    out.push_str("  EntityTemplateResourceClass PC {\n");
    out.push_str("  }\n");
    out.push_str("  EntityTemplateResourceClass XBOX_ONE : PC {\n");
    out.push_str("  }\n");
    out.push_str("  EntityTemplateResourceClass XBOX_SERIES : PC {\n");
    out.push_str("  }\n");
    out.push_str("  EntityTemplateResourceClass PS4 : PC {\n");
    out.push_str("  }\n");
    out.push_str("  EntityTemplateResourceClass PS5 : PC {\n");
    out.push_str("  }\n");
    out.push_str("  EntityTemplateResourceClass HEADLESS : PC {\n");
    out.push_str("  }\n");
    out.push_str(" }\n");
    out.push_str("}\n");
    out
}

fn rel_from_known_roots(abs_path: &Path) -> String {
    let p = abs_path.to_string_lossy().replace('\\', "/");
    for anchor in ["/Prefabs/", "/prefabs/", "/Assets/", "/assets/"] {
        if let Some(idx) = p.find(anchor) {
            return p[(idx + 1)..].to_string();
        }
    }
    abs_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| p)
}

fn resolve_et_save_path(xob_abs: &Path, save_dir: Option<&str>) -> Result<PathBuf, String> {
    let base = xob_abs
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Invalid xob file name".to_string())?;
    let auto_name = format!("{}_test_prefab.et", base);
    let dir = if let Some(sd) = save_dir {
        let pb = PathBuf::from(sd);
        if pb.is_dir() {
            pb
        } else {
            xob_abs
                .parent()
                .ok_or_else(|| "Invalid xob directory".to_string())?
                .to_path_buf()
        }
    } else {
        xob_abs
            .parent()
            .ok_or_else(|| "Invalid xob directory".to_string())?
            .to_path_buf()
    };
    Ok(dir.join(auto_name))
}

fn read_xob_object_field_from_meta(xob_abs: &Path) -> Result<(String, String), String> {
    let meta_path = PathBuf::from(format!("{}.meta", xob_abs.to_string_lossy()));
    let text = fs::read_to_string(&meta_path)
        .map_err(|e| format!("Failed to read .xob.meta: {}", e))?;
    let needle = "Name \"";
    let idx = text
        .find(needle)
        .ok_or_else(|| "Name field not found in .xob.meta".to_string())?;
    let rest = &text[idx + needle.len()..];
    let end_idx = rest
        .find('"')
        .ok_or_else(|| "Invalid Name field in .xob.meta".to_string())?;
    let name_value = rest[..end_idx].trim().to_string();
    // Expect "{GUID}Assets/...xob"
    let guid = extract_guid(&name_value).ok_or_else(|| "GUID not found in .xob.meta Name".to_string())?;
    let rel_assets = if let Some(close) = name_value.find('}') {
        name_value[(close + 1)..].to_string()
    } else {
        name_value.clone()
    };
    Ok((guid, rel_assets))
}

fn parse_txo_socket_names(txo_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let s = txo_text.as_bytes();
    while i < s.len() {
        // naive scan for $node "socket_
        if s[i..].starts_with(b"$node\"") || s[i..].starts_with(b"$node \"") {
            // find first quote
            let q1 = txo_text[i..].find('"').map(|v| i + v);
            if let Some(q1) = q1 {
                let q2 = txo_text[(q1 + 1)..].find('"').map(|v| q1 + 1 + v);
                if let Some(q2) = q2 {
                    let name = txo_text[(q1 + 1)..q2].trim();
                    if name.to_lowercase().starts_with("socket_") {
                        out.push(name.to_string());
                    }
                    i = q2 + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn prefab_candidates_from_socket(socket_name: &str) -> Vec<String> {
    let s = socket_name.trim().to_lowercase();
    if !s.starts_with("socket_") {
        return vec![];
    }
    let mut tail = s["socket_".len()..].to_string();
    if let Some(pos) = tail.find(".et") {
        tail = tail[..pos].to_string();
    }
    let mut tail = tail.replace('.', "_");
    while tail.contains("__") {
        tail = tail.replace("__", "_");
    }
    tail = tail.trim_matches('_').to_string();
    let mut cands: Vec<String> = Vec::new();
    let mut push = |v: String| {
        if !v.is_empty() && !cands.contains(&v) {
            cands.push(v);
        }
    };
    push(tail.clone());
    // strip trailing _<digits>
    if let Some(pos) = tail.rfind('_') {
        if tail[(pos + 1)..].chars().all(|c| c.is_ascii_digit()) {
            push(tail[..pos].to_string());
        }
    }
    cands
}

fn extract_guid_from_socket_name(name: &str) -> Option<String> {
    let s = name.trim();
    let lower = s.to_lowercase();
    if !lower.starts_with("socket_") {
        return None;
    }
    let tail = &lower["socket_".len()..];
    let guid = tail.get(0..16)?;
    if guid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(guid.to_uppercase());
    }
    None
}

fn load_prefab_index_maps(
) -> Result<(
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
), String> {
    let path = prefab_index_path();
    let text = fs::read_to_string(&path).map_err(|e| format!("Failed to read prefab cache: {}", e))?;
    let value = serde_json::from_str::<JsonValue>(&text).map_err(|e| e.to_string())?;
    let mut name_index: BTreeMap<String, String> = BTreeMap::new();
    let mut guid_index: BTreeMap<String, String> = BTreeMap::new();
    let mut et_path_index: BTreeMap<String, String> = BTreeMap::new();
    let mut guid_path_index: BTreeMap<String, String> = BTreeMap::new();
    if let Some(obj) = value.get("name_index").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                name_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    if let Some(obj) = value.get("guid_index").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                guid_index.insert(k.to_string(), s.to_string());
            }
        }
    }

    if let Some(obj) = value.get("et_path_index").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                et_path_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    if let Some(obj) = value.get("guid_path_index").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                guid_path_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    Ok((name_index, guid_index, et_path_index, guid_path_index))
}

fn find_in_path(exe_name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for p in std::env::split_paths(&path) {
        let cand = p.join(exe_name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn resolve_blender_path() -> Option<PathBuf> {
    for envk in ["OWLTOOLS_BLENDER_PATH", "BLENDER_PATH"] {
        if let Ok(v) = std::env::var(envk) {
            let p = PathBuf::from(v);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // PATH
    if let Some(exe) = find_in_path("blender.exe") {
        return Some(exe);
    }
    if let Some(exe) = find_in_path("blender") {
        return Some(exe);
    }
    // Common Windows locations
    for c in [
        r"C:\Program Files\Blender Foundation\Blender\blender.exe",
        r"C:\Program Files\Blender Foundation\Blender 4.3\blender.exe",
        r"C:\Program Files\Blender Foundation\Blender 4.2\blender.exe",
        r"C:\Program Files\Blender Foundation\Blender 4.1\blender.exe",
        r"C:\Program Files\Blender Foundation\Blender 4.0\blender.exe",
    ] {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn normalize_socket_key(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .replace('-', "_")
        .replace(' ', "_")
}

fn extract_socket_guids_with_blender(app: &tauri::AppHandle, xob_abs: &Path) -> Option<BTreeMap<String, String>> {
    let fbx_abs = xob_abs.with_extension("fbx");
    if !fbx_abs.is_file() {
        emit_scan_log(app, "info", format!("Blender GUID match: FBX not found: {}", fbx_abs.to_string_lossy()), None, None);
        return None;
    }
    let settings = load_settings();
    let blender = settings
        .blender_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(resolve_blender_path);
    let blender = match blender {
        Some(p) => p,
        None => {
            emit_scan_log(
                app,
                "info",
                "Blender GUID match: blender.exe not configured (set OWLTOOLS_BLENDER_PATH) — skip",
                None,
                None,
            );
            return None;
        }
    };
    emit_scan_log(
        app,
        "info",
        format!("Blender GUID match: running {}", blender.to_string_lossy()),
        None,
        None,
    );

    let py = format!(
        r#"import bpy, json, re
fbx=r'''{}'''
try:
    bpy.ops.wm.read_factory_settings(use_empty=True)
except Exception:
    pass
try:
    bpy.ops.import_scene.fbx(filepath=fbx, automatic_bone_orientation=True)
except Exception:
    print("{{}}")
    raise
data={{}}
for ob in bpy.data.objects:
    n=(ob.name or '')
    if not n.lower().startswith('socket'):
        continue
    guid=None
    try:
        guid = ob.get('ref_guid') or ob.get('ref guid')
    except Exception:
        guid=None
    if not guid:
        try:
            for k in ob.keys():
                kk=str(k).lower().replace(' ','').replace('_','')
                if kk=='refguid':
                    v=ob.get(k)
                    guid=v if isinstance(v,str) else None
                    break
        except Exception:
            pass
    if isinstance(guid,str):
        m=re.search(r'([0-9A-Fa-f]{{16}})', guid)
        guid=m.group(1).upper() if m else None
    if guid:
        data[n]=guid
print(json.dumps(data))
"#,
        fbx_abs.to_string_lossy()
    );

    let out = std::process::Command::new(blender)
        .args(["--background", "--factory-startup", "--python-expr", &py])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !stderr.trim().is_empty() {
        emit_scan_log(app, "warn", format!("Blender GUID match stderr: {}", stderr.trim()), None, None);
    }

    // Blender prints multiple lines; find the last JSON-like line
    let mut json_line: Option<String> = None;
    for line in stdout.lines().rev() {
        let t = line.trim();
        if t.starts_with('{') && t.ends_with('}') {
            json_line = Some(t.to_string());
            break;
        }
    }
    let json_line = json_line?;
    let v: serde_json::Value = serde_json::from_str(&json_line).ok()?;
    let obj = v.as_object()?;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (k, val) in obj {
        if let Some(g) = val.as_str() {
            map.insert(k.to_string(), g.to_string());
        }
    }
    if !map.is_empty() {
        emit_scan_log(app, "info", format!("Blender GUID match: extracted {} ref_guid", map.len()), None, None);
    }
    Some(map)
}

fn build_slot_component_text_from_mappings(maps: &Vec<(String, String)>) -> Option<String> {
    if maps.is_empty() {
        return None;
    }
    let slot_guid = gen_hex16();
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("  WB_SlotBoneMappingsComponent \"{{{}}}\" {{", slot_guid));
    lines.push("   SlotBoneMappings {".to_string());
    for (bone_prefix, prefab_name) in maps {
        let obj_guid = gen_hex16();
        lines.push(format!("    SlotBoneMappingObject \"{{{}}}\" {{", obj_guid));
        lines.push(format!("     BonePrefix \"{}\"", bone_prefix));
        lines.push(format!("     Prefab \"{}\"", prefab_name));
        lines.push("    }".to_string());
    }
    lines.push("   }".to_string());
    lines.push("  }".to_string());
    Some(lines.join("\n") + "\n")
}

fn build_child_entities_block(maps: &Vec<(String, String)>, hier_guid: &str) -> Option<String> {
    if maps.is_empty() {
        return None;
    }
    let mut by_prefab: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (bone_prefix, prefab_name) in maps {
        by_prefab.entry(prefab_name.clone()).or_default().push(bone_prefix.clone());
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push(" {".to_string());
    for (prefab, sockets) in by_prefab {
        if sockets.len() >= 2 {
            lines.push(format!("  $grp GenericEntity : \"{}\" {{", prefab));
            for s in sockets {
                let inst_id = gen_hex16();
                lines.push("   {".to_string());
                lines.push(format!("    ID \"{}\"", inst_id));
                lines.push("    components {".to_string());
                lines.push(format!("     Hierarchy \"{{{}}}\" {{", hier_guid));
                lines.push("      Enabled 1".to_string());
                lines.push(format!("      PivotID \"{}\"", s));
                lines.push("      AutoTransform 1".to_string());
                lines.push("     }".to_string());
                lines.push("    }".to_string());
                lines.push("    coords 0 0 0".to_string());
                lines.push("   }".to_string());
            }
            lines.push("  }".to_string());
        } else {
            let socket_name = sockets.get(0).cloned().unwrap_or_default();
            let inst_id = gen_hex16();
            lines.push(format!("  GenericEntity : \"{}\" {{", prefab));
            lines.push(format!("   ID \"{}\"", inst_id));
            lines.push("   components {".to_string());
            lines.push(format!("    Hierarchy \"{{{}}}\" {{", hier_guid));
            lines.push("     Enabled 1".to_string());
            lines.push(format!("     PivotID \"{}\"", socket_name));
            lines.push("     AutoTransform 1".to_string());
            lines.push("    }".to_string());
            lines.push("   }".to_string());
            lines.push("   coords 0 0 0".to_string());
            lines.push("  }".to_string());
        }
    }
    lines.push(" }".to_string());
    Some(lines.join("\n") + "\n")
}

fn build_new_et_with_mesh(gen_id: &str, obj_field: &str) -> String {
    let mesh_guid = gen_hex16();
    format!(
        "GenericEntity {{\n ID \"{}\"\n components {{\n  MeshObject \"{{{}}}\" {{\n   Object \"{}\"\n  }}\n }}\n coords 0 0 0\n}}\n",
        gen_id, mesh_guid, obj_field
    )
}

fn remove_blank_lines(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn insert_or_replace_slot_component(mut et_text: String, component_text: &str) -> String {
    if let Some(pos) = et_text.find(" components {") {
        // Insert before closing of components block " }" by finding next "}\n coords"
        if let Some(cpos) = et_text[pos..].find("}\n coords") {
            let abs = pos + cpos;
            et_text.insert_str(abs, &format!("\n{}", component_text));
            return et_text;
        }
    }
    // Fallback append
    et_text.push_str("\n");
    et_text.push_str(component_text);
    et_text
}

fn insert_or_replace_child_entities_block(mut et_text: String, block_text: &str) -> String {
    if let Some(pos) = et_text.find("coords 0 0 0") {
        if let Some(endline) = et_text[pos..].find('\n') {
            let insert_at = pos + endline;
            et_text.insert_str(insert_at, &format!("\n{}", block_text));
            return et_text;
        }
    }
    et_text.push_str("\n");
    et_text.push_str(block_text);
    et_text
}

#[derive(Serialize)]
struct CreateEtResult {
    et_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta_path: Option<String>,
    sockets: usize,
    matched: usize,
    unmatched: usize,
    suggested_extra_dirs: Vec<String>,
}

#[derive(Serialize)]
struct SuggestFoldersResult {
    sockets: usize,
    matched: usize,
    unmatched: usize,
    suggested_extra_dirs: Vec<String>,
}

#[tauri::command]
async fn suggest_prefab_folders_from_xob(
    app: tauri::AppHandle,
    xob_path: String,
    svn_root: Option<String>,
    extra_dirs: Option<Vec<String>>,
) -> Result<SuggestFoldersResult, String> {
    let xob_abs = PathBuf::from(&xob_path);
    if !xob_abs.is_file() {
        return Err(format!("Invalid .xob path: {}", xob_path));
    }
    emit_scan_log(&app, "info", format!("Detect folders for xob: {}", xob_abs.to_string_lossy()), None, None);

    let txo_abs = xob_abs.with_extension("txo");
    let sockets: Vec<String> = if txo_abs.is_file() {
        let txo_text = fs::read_to_string(&txo_abs)
            .map_err(|e| format!("Failed to read .txo: {}", e))?;
        parse_txo_socket_names(&txo_text)
    } else {
        Vec::new()
    };

    let (name_index, guid_index, et_path_index, guid_path_index) = load_prefab_index_maps()?;
    let _svn_root = svn_root
        .or_else(|| load_settings().svn_root)
        .filter(|s| !s.is_empty());
    let extra_dirs = extra_dirs
        .or_else(|| load_settings().extra_dirs)
        .unwrap_or_default();

    let total_s = sockets.len();
    let blender_sock_guids = extract_socket_guids_with_blender(&app, &xob_abs);
    let blender_sock_guids_lc: Option<BTreeMap<String, String>> = blender_sock_guids.as_ref().map(|m| {
        m.iter()
            .map(|(k, v)| (normalize_socket_key(k), v.clone()))
            .collect::<BTreeMap<String, String>>()
    });

    // Match sockets similarly to create_new_et_from_xob
    let mut matched_sock: Vec<String> = Vec::new();
    for s in &sockets {
        let mut found = false;
        if let Some(bg) = blender_sock_guids_lc.as_ref() {
            if let Some(g) = bg.get(&normalize_socket_key(s)) {
                if guid_index.contains_key(g) {
                    found = true;
                }
            }
        }
        if !found {
            if let Some(g) = extract_guid_from_socket_name(s) {
                if guid_index.contains_key(&g) {
                    found = true;
                }
            }
        }
        if !found {
            for cand in prefab_candidates_from_socket(s) {
                let key = format!("{}.et", cand);
                if name_index.contains_key(&key) {
                    found = true;
                    break;
                }
            }
        }
        if found {
            matched_sock.push(s.clone());
        }
    }

    let matched = matched_sock.len();
    let unmatched = total_s.saturating_sub(matched);

    let mut suggested: BTreeSet<String> = BTreeSet::new();
    for sock in &matched_sock {
        let mut hit_path: Option<String> = None;
        if let Some(bg) = blender_sock_guids_lc.as_ref() {
            if let Some(g) = bg.get(&normalize_socket_key(sock)) {
                if let Some(p) = guid_path_index.get(g) {
                    hit_path = Some(p.clone());
                }
            }
        }
        if hit_path.is_none() {
            if let Some(g) = extract_guid_from_socket_name(sock) {
                if let Some(p) = guid_path_index.get(&g) {
                    hit_path = Some(p.clone());
                }
            }
        }
        if hit_path.is_none() {
            for cand in prefab_candidates_from_socket(sock) {
                let key = format!("{}.et", cand);
                if let Some(p) = et_path_index.get(&key) {
                    hit_path = Some(p.clone());
                    break;
                }
            }
        }
        if let Some(p) = hit_path {
            let pb = PathBuf::from(p);
            if let Some(dir) = pb.parent() {
                let dir_s = dir.to_string_lossy().to_string();
                if extra_dirs.contains(&dir_s) {
                    continue;
                }
                // IMPORTANT: show detected folders even if they are under SVN root.
                // This matches the EnfAutoSocket UX where users can see/choose prefab folders explicitly.
                suggested.insert(dir_s);
            }
        }
    }
    let suggested_extra_dirs: Vec<String> = suggested.into_iter().collect();
    emit_scan_log(
        &app,
        "info",
        format!("Detected prefab folders: {}", suggested_extra_dirs.len()),
        None,
        None,
    );

    Ok(SuggestFoldersResult {
        sockets: total_s,
        matched,
        unmatched,
        suggested_extra_dirs,
    })
}

#[tauri::command]
async fn create_new_et_from_xob(
    app: tauri::AppHandle,
    xob_path: String,
    save_dir: Option<String>,
    svn_root: Option<String>,
    extra_dirs: Option<Vec<String>>,
) -> Result<CreateEtResult, String> {
    let xob_abs = PathBuf::from(&xob_path);
    if !xob_abs.is_file() {
        let msg = format!("Invalid .xob path: {}", xob_path);
        emit_scan_log(&app, "error", msg.clone(), None, None);
        return Err(msg);
    }

    emit_scan_log(&app, "info", format!("Processing xob: {}", xob_abs.to_string_lossy()), None, None);

    let (obj_guid, obj_path) = read_xob_object_field_from_meta(&xob_abs)?;
    let obj_field = format!("{{{}}}{}", obj_guid, obj_path);
    emit_scan_log(&app, "info", "Loaded .xob.meta Name/GUID", None, None);

    let txo_abs = xob_abs.with_extension("txo");
    let sockets: Vec<String> = if txo_abs.is_file() {
        let txo_text = fs::read_to_string(&txo_abs)
            .map_err(|e| format!("Failed to read .txo: {}", e))?;
        let out = parse_txo_socket_names(&txo_text);
        emit_scan_log(&app, "info", format!("Found {} sockets", out.len()), None, None);
        out
    } else {
        emit_scan_log(&app, "warn", "No .txo found (no sockets)", None, None);
        Vec::new()
    };

    let (name_index, guid_index, et_path_index, guid_path_index) = load_prefab_index_maps()?;
    emit_scan_log(
        &app,
        "info",
        format!(
            "Loaded prefab cache (names={}, guids={}, et_paths={}, guid_paths={})",
            name_index.len(),
            guid_index.len(),
            et_path_index.len(),
            guid_path_index.len()
        ),
        None,
        None,
    );

    let svn_root = svn_root
        .or_else(|| load_settings().svn_root)
        .filter(|s| !s.is_empty());
    let extra_dirs = extra_dirs
        .or_else(|| load_settings().extra_dirs)
        .unwrap_or_default();
    if !extra_dirs.is_empty() {
        emit_scan_log(&app, "info", format!("Extra dirs: {}", extra_dirs.len()), None, None);
    }

    let total_s = sockets.len();

    // Blender GUID extraction (optional)
    let blender_sock_guids = extract_socket_guids_with_blender(&app, &xob_abs);
    let blender_sock_guids_lc: Option<BTreeMap<String, String>> = blender_sock_guids.as_ref().map(|m| {
        m.iter()
            .map(|(k, v)| (normalize_socket_key(k), v.clone()))
            .collect::<BTreeMap<String, String>>()
    });
    if let Some(m) = blender_sock_guids.as_ref() {
        for (k, v) in m.iter().take(6) {
            emit_scan_log(&app, "info", format!("Blender GUID: {} -> {}", k, v), None, None);
        }
    }

    if let Some(bg) = blender_sock_guids_lc.as_ref() {
        emit_scan_log(
            &app,
            "info",
            format!("Blender GUID keys: {} (normalized)", bg.len()),
            None,
            None,
        );
        for s in sockets.iter().take(5) {
            let nk = normalize_socket_key(s);
            let hit = bg.contains_key(&nk);
            emit_scan_log(
                &app,
                "info",
                format!("Socket key check: '{}' -> '{}' | in_blender={}", s, nk, hit),
                None,
                None,
            );
        }
    }

    let mut maps: Vec<(String, String)> = Vec::new();
    for (i, s) in sockets.iter().enumerate() {
        if total_s > 0 {
            emit_scan_log(&app, "info", "Matching sockets...", Some(i), Some(total_s));
        }
        let mut prefab_meta_name: Option<String> = None;
        // 1) GUID from Blender ref_guid (best)
        if let Some(bg) = blender_sock_guids_lc.as_ref() {
            if let Some(guid) = bg.get(&normalize_socket_key(s)) {
                if let Some(v) = guid_index.get(guid) {
                    prefab_meta_name = Some(v.clone());
                }
            }
        }
        // 2) GUID embedded in socket name
        if prefab_meta_name.is_none() {
            if let Some(guid) = extract_guid_from_socket_name(s) {
                if let Some(v) = guid_index.get(&guid) {
                    prefab_meta_name = Some(v.clone());
                }
            }
        }
        if prefab_meta_name.is_none() {
            for cand in prefab_candidates_from_socket(s) {
                let key = format!("{}.et", cand);
                if let Some(v) = name_index.get(&key) {
                    prefab_meta_name = Some(v.clone());
                    break;
                }
            }
        }
        if let Some(prefab) = prefab_meta_name {
            maps.push((s.clone(), prefab));
        }
    }
    if total_s > 0 {
        emit_scan_log(&app, "info", "Matching sockets...", Some(total_s), Some(total_s));
    }

    let matched = maps.len();
    let unmatched = total_s.saturating_sub(matched);
    emit_scan_log(&app, "info", format!("Matched sockets: {}/{}", matched, total_s), None, None);

    // Auto-suggest extra dirs based on matched prefab absolute paths in cache
    let mut suggested: BTreeSet<String> = BTreeSet::new();
    let svn_norm = svn_root.as_ref().map(|s| PathBuf::from(s).to_string_lossy().to_string().to_lowercase());
    for (sock, _) in &maps {
        // Prefer Blender ref_guid path, else embedded guid, else name-index path
        let mut hit_path: Option<String> = None;
        if let Some(bg) = blender_sock_guids_lc.as_ref() {
            if let Some(g) = bg.get(&normalize_socket_key(sock)) {
                if let Some(p) = guid_path_index.get(g) {
                    hit_path = Some(p.clone());
                }
            }
        }
        if hit_path.is_none() {
            if let Some(g) = extract_guid_from_socket_name(sock) {
                if let Some(p) = guid_path_index.get(&g) {
                    hit_path = Some(p.clone());
                }
            }
        }
        if hit_path.is_none() {
            for cand in prefab_candidates_from_socket(sock) {
                let key = format!("{}.et", cand);
                if let Some(p) = et_path_index.get(&key) {
                    hit_path = Some(p.clone());
                    break;
                }
            }
        }
        if let Some(p) = hit_path {
            let pb = PathBuf::from(p);
            if let Some(dir) = pb.parent() {
                let dir_s = dir.to_string_lossy().to_string();
                let dir_l = dir_s.to_lowercase();
                let under_svn = svn_norm.as_ref().map(|svn| dir_l.starts_with(svn)).unwrap_or(false);
                if !under_svn {
                    suggested.insert(dir_s);
                }
            }
        }
    }
    let suggested_extra_dirs: Vec<String> = suggested.into_iter().collect();
    if !suggested_extra_dirs.is_empty() {
        emit_scan_log(
            &app,
            "info",
            format!("Auto-assign prefab folders: {}", suggested_extra_dirs.len()),
            None,
            None,
        );
    }

    let out_path = resolve_et_save_path(&xob_abs, save_dir.as_deref())?;
    if let Some(sd) = save_dir.as_deref() {
        remember_save_dir(Some(sd.to_string())).ok();
    }
    // Persist extra dirs union (existing + suggested)
    let mut merged_extra = extra_dirs.clone();
    for p in &suggested_extra_dirs {
        if !merged_extra.contains(p) {
            merged_extra.push(p.clone());
        }
    }
    if !merged_extra.is_empty() {
        remember_extra_dirs(merged_extra.clone()).ok();
    }
    if let Some(sr) = svn_root.as_deref() {
        remember_svn_root(Some(sr.to_string())).ok();
    }

    let gen_id = gen_hex16();
    let mut et_text = build_new_et_with_mesh(&gen_id, &obj_field);
    if let Some(comp) = build_slot_component_text_from_mappings(&maps) {
        et_text = insert_or_replace_slot_component(et_text, &comp);
    }
    let hier_guid = gen_hex16();
    if let Some(child) = build_child_entities_block(&maps, &hier_guid) {
        et_text = insert_or_replace_child_entities_block(et_text, &child);
    }
    et_text = remove_blank_lines(&et_text);

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    emit_scan_log(&app, "info", format!("Writing .et: {}", out_path.to_string_lossy()), None, None);
    fs::write(&out_path, et_text).map_err(|e| e.to_string())?;
    emit_scan_log(&app, "info", "Process finished", None, None);

    Ok(CreateEtResult {
        et_path: out_path.to_string_lossy().to_string(),
        meta_path: None,
        sockets: total_s,
        matched,
        unmatched,
        suggested_extra_dirs,
    })
}

#[tauri::command]
async fn create_new_et_with_meta_from_xob(
    app: tauri::AppHandle,
    xob_path: String,
    save_dir: Option<String>,
    svn_root: Option<String>,
    extra_dirs: Option<Vec<String>>,
) -> Result<CreateEtResult, String> {
    let mut res = create_new_et_from_xob(app.clone(), xob_path, save_dir, svn_root, extra_dirs).await?;
    let et_abs = PathBuf::from(&res.et_path);
    let rel = rel_from_known_roots(&et_abs);
    let name_value = format!("{{{}}}{}", gen_hex16(), rel);
    let meta_text = build_et_meta_text(&name_value);
    let meta_path = PathBuf::from(format!("{}.meta", et_abs.to_string_lossy()));
    emit_scan_log(&app, "info", format!("Writing .et.meta: {}", meta_path.to_string_lossy()), None, None);
    fs::write(&meta_path, meta_text).map_err(|e| e.to_string())?;

    // Incrementally update prefab cache so the new .et is available immediately for matching/suggestions.
    if let Err(err) = update_prefab_cache_with_new_meta(&et_abs, &meta_path, &name_value) {
        emit_scan_log(&app, "warn", format!("Failed to update prefab cache: {}", err), None, None);
    } else {
        emit_scan_log(&app, "info", "Updated prefab cache", None, None);
    }

    res.meta_path = Some(meta_path.to_string_lossy().to_string());
    Ok(res)
}

fn emit_scan_log(app: &tauri::AppHandle, level: &str, message: impl Into<String>, current: Option<usize>, total: Option<usize>) {
    let payload = ScanLogPayload {
        level: level.to_string(),
        message: message.into(),
        current,
        total,
    };
    let _ = app.emit("prefab_scan_log", payload);
}

fn emit_prefabdst_log(app: &tauri::AppHandle, level: &str, message: impl Into<String>, current: Option<usize>, total: Option<usize>) {
    let payload = PrefabDstLogPayload {
        level: level.to_string(),
        message: message.into(),
        current,
        total,
    };
    let _ = app.emit("prefabdst_log", payload);
}

fn detect_newline(s: &str) -> &'static str {
    if s.contains("\r\n") { "\r\n" } else { "\n" }
}

fn strip_preset_header(body: &str) -> String {
    // Remove leading "KEY: value" lines until first blank line.
    let nl = detect_newline(body);
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = true;
    let re_kv = Regex::new(r"^\s*[A-Za-z_][A-Za-z0-9_\-]*\s*:\s*.*$").unwrap();
    for ln in body.lines() {
        if skipping {
            if ln.trim().is_empty() {
                skipping = false;
                continue;
            }
            if re_kv.is_match(ln) {
                continue;
            }
            // Unexpected line: treat as start of body
            skipping = false;
        }
        out.push(ln);
    }
    out.join(nl)
}

fn replace_object_marker(text: &str, guid: &str, res: &str) -> String {
    // Replace annotated marker line like: Object "{guid from xob input}path from xob input"
    let rep = format!("Object \"{{{}}}{}\"", guid, res);
    let re = Regex::new(r#"Object\s+"[^"\n]*guid\s+from\s+xob\s+input[^"\n]*""#).unwrap();
    re.replace_all(text, rep).to_string()
}

fn replace_v2_model_marker(text: &str, guid: &str, res: &str) -> String {
    let rep = format!("Model \"{{{}}}{}\"", guid, res);
    let re = Regex::new(r#"Model\s+"[^"\n]*same\s+name_v2_dst[^"\n]*""#).unwrap();
    re.replace_all(text, rep).to_string()
}

fn replace_gen_guid_markers(text: &str) -> String {
    let re_braced = Regex::new(r"\{\s*gen\s+guid\s*\}").unwrap();
    let re_quoted = Regex::new(r#""\s*gen\s+guid\s*""#).unwrap();
    let s = re_braced
        .replace_all(text, |_caps: &regex::Captures| format!("{{{}}}", gen_guid16()))
        .to_string();
    re_quoted
        .replace_all(&s, |_caps: &regex::Captures| format!("\"{}\"", gen_guid16()))
        .to_string()
}

fn replace_gen_vec3_markers(text: &str) -> String {
    // The UI/build pipeline should not generate random vec3 values.
    // Default to 0 0 0 for any {gen vec3} marker.
    let re = Regex::new(r"\{\s*gen\s+vec3\s*\}").unwrap();
    re.replace_all(text, "0 0 0").to_string()
}

fn find_fractalparts_bounds(text: &str) -> Option<(usize, usize, usize)> {
    // Returns (open_brace_idx, close_brace_idx, keyword_idx)
    let kw = text.find("FractalParts")?;
    let open = text[kw..].find('{').map(|i| kw + i)?;
    let mut depth: i32 = 0;
    let mut i = open;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        match bytes[i] as char {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, i, kw));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn extract_fractalpart_blocks(inner: &str) -> (Vec<(char, String)>, Vec<String>) {
    // Parse by lines with brace counting, matching the Python approach.
    let lines: Vec<&str> = inner.lines().collect();
    let mut blocks: Vec<(char, String)> = Vec::new();
    let mut others: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let ln = lines[i];
        if ln.trim_start().starts_with("FractalPartData") {
            let mut depth: i32 = 0;
            let mut j = i;
            let mut started = false;
            while j < lines.len() {
                let l2 = lines[j];
                if l2.contains('{') {
                    started = true;
                    depth += l2.matches('{').count() as i32;
                }
                if l2.contains('}') {
                    depth -= l2.matches('}').count() as i32;
                }
                j += 1;
                if started && depth <= 0 {
                    break;
                }
            }
            let nl = detect_newline(inner);
            let blk = lines[i..j].join(nl);
            let letter = Regex::new(r##"\bPartId\s+\"([A-Z])\""##)
                .unwrap()
                .captures(&blk)
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().chars().next())
                .unwrap_or('A');
            blocks.push((letter, blk));
            i = j;
        } else {
            others.push(ln.to_string());
            i += 1;
        }
    }
    blocks.sort_by_key(|(c, _)| *c as u32);
    (blocks, others)
}

fn clone_fractalpart_block(block: &str, src_letter: char, new_letter: char) -> String {
    let mut out = block.to_string();
    let re_partid = Regex::new(&format!(r##"\bPartId\s+\"{}\""##, regex::escape(&src_letter.to_string()))).unwrap();
    out = re_partid.replace_all(&out, format!("PartId \"{}\"", new_letter)).to_string();

    let re_q_letter = Regex::new(&format!(r##"\"{}\""##, regex::escape(&src_letter.to_string()))).unwrap();
    out = re_q_letter.replace_all(&out, format!("\"{}\"", new_letter)).to_string();

    let re_q_letter_num = Regex::new(&format!(r##"\"{}(\d+)\""##, regex::escape(&src_letter.to_string()))).unwrap();
    out = re_q_letter_num
        .replace_all(&out, |caps: &regex::Captures| format!("\"{}{}\"", new_letter, &caps[1]))
        .to_string();

    let re_guid = Regex::new(r##"\"\{[0-9A-Fa-f]{16}\}\""##).unwrap();
    out = re_guid
        .replace_all(&out, |_caps: &regex::Captures| format!("\"{{{}}}\"", gen_guid16()))
        .to_string();

    // Also update template markers that encode the part letter.
    // Example: {{DEBRIS_ID-A}} / {{COLLIDERS_ID-A}}
    out = out.replace(
        &format!("{{{{DEBRIS_ID-{}}}}}", src_letter),
        &format!("{{{{DEBRIS_ID-{}}}}}", new_letter)
    );
    out = out.replace(
        &format!("{{{{COLLIDERS_ID-{}}}}}", src_letter),
        &format!("{{{{COLLIDERS_ID-{}}}}}", new_letter)
    );
    out
}

fn build_debris_infos_block(items: &[MetaEntry], indent: &str) -> String {
    // Build a list of SCR_DebrisInfo blocks.
    // We don't currently have offsets/mass; use neutral defaults.
    if items.is_empty() {
        return format!("{}", "");
    }
    let mut lines: Vec<String> = Vec::new();
    for e in items {
        let info_id = gen_guid16();
        let ap_id = gen_guid16();
        lines.push(format!("{}SCR_DebrisInfo \"{{{}}}\" {{", indent, info_id));
        lines.push(format!("{} ModelPrefab \"{{{}}}{}\"", indent, e.guid, e.path));
        lines.push(format!("{} LocalTransform AttachPoint \"{{{}}}\" {{", indent, ap_id));
        lines.push(format!("{}  Offset 0 0 0", indent));
        lines.push(format!("{}  Angles 0.00001 0 0", indent));
        lines.push(format!("{} }}", indent));
        lines.push(format!("{} m_fMass 500", indent));
        lines.push(format!("{}}}", indent));
    }
    lines.join("\n")
}

fn build_colliders_line(tags: &[String]) -> String {
    if tags.is_empty() {
        return "\"\"".to_string();
    }
    tags.iter()
        .map(|t| format!("\"{}\"", t))
        .collect::<Vec<String>>()
        .join(" ")
}

fn replace_full_dst_markers(text: &str, scan: &FullDstScanResult) -> String {
    // Replace {{DEBRIS_ID-X}} and {{COLLIDERS_ID-X}} markers.
    let mut by_part: HashMap<String, (Vec<MetaEntry>, Vec<String>)> = HashMap::new();
    for z in &scan.zones {
        by_part.insert(z.part_id.clone(), (z.debris.clone(), z.colliders.clone()));
    }

    let re_debris = Regex::new(r"(?m)^(?P<indent>\s*)\{\{DEBRIS_ID-(?P<p>[A-Z])\}\}\s*$").unwrap();
    let re_cols = Regex::new(r"(?m)^(?P<indent>\s*)\{\{COLLIDERS_ID-(?P<p>[A-Z])\}\}\s*$").unwrap();

    let s = re_debris
        .replace_all(text, |caps: &regex::Captures| {
            let indent = caps.name("indent").map(|m| m.as_str()).unwrap_or("");
            let pid = caps.name("p").map(|m| m.as_str()).unwrap_or("A");
            let (debris, _cols) = by_part.get(pid).cloned().unwrap_or_default();
            // Indent children one extra space relative to marker line.
            let child_indent = format!("{} ", indent);
            build_debris_infos_block(&debris, &child_indent)
        })
        .to_string();

    re_cols
        .replace_all(&s, |caps: &regex::Captures| {
            let indent = caps.name("indent").map(|m| m.as_str()).unwrap_or("");
            let pid = caps.name("p").map(|m| m.as_str()).unwrap_or("A");
            let (_debris, cols) = by_part.get(pid).cloned().unwrap_or_default();
            format!("{}{}", indent, build_colliders_line(&cols))
        })
        .to_string()
}

fn ensure_fractalparts_zone_count(text: &str, desired_count: usize) -> String {
    let desired = desired_count.clamp(1, 26);
    let nl = detect_newline(text);
    let Some((open, close, _kw)) = find_fractalparts_bounds(text) else {
        return text.to_string();
    };
    let inner = &text[(open + 1)..close];
    let (blocks, other_lines) = extract_fractalpart_blocks(inner);

    let mut existing: HashMap<char, String> = HashMap::new();
    for (c, b) in blocks.iter() {
        existing.insert(*c, b.clone());
    }

    let mut template_letter: Option<char> = None;
    let mut template_block: Option<String> = None;
    for (c, b) in blocks.iter().rev() {
        if Regex::new(r##"\bPartId\s+\"([A-Z])\""##).unwrap().is_match(b) {
            template_letter = Some(*c);
            template_block = Some(b.clone());
            break;
        }
    }
    if template_block.is_none() {
        if let Some((c, b)) = blocks.last() {
            template_letter = Some(*c);
            template_block = Some(b.clone());
        }
    }

    let mut kept: Vec<String> = Vec::new();
    for i in 0..desired {
        let lt = (b'A' + (i as u8)) as char;
        if let Some(b) = existing.get(&lt) {
            kept.push(b.clone());
        } else if let (Some(tb), Some(tl)) = (template_block.as_ref(), template_letter) {
            kept.push(clone_fractalpart_block(tb, tl, lt));
        }
    }

    let mut inner_new_lines: Vec<String> = Vec::new();
    for blk in kept {
        inner_new_lines.extend(blk.lines().map(|s| s.to_string()));
    }
    if other_lines.iter().any(|s| !s.trim().is_empty()) {
        inner_new_lines.extend(other_lines);
    }
    let new_inner = inner_new_lines.join(nl);
    format!("{}{}{}{}{}", &text[..(open + 1)], nl, new_inner, nl, &text[close..])
}

fn apply_zone_hp(text: &str, hp: i32) -> String {
    let nl = detect_newline(text);
    let Some((open, close, _kw)) = find_fractalparts_bounds(text) else {
        return text.to_string();
    };
    let inner = &text[(open + 1)..close];
    let (blocks, other_lines) = extract_fractalpart_blocks(inner);
    let re_hp = Regex::new(r"(?m)^(\s*MaxHealth)\s+[0-9]+(?:\.[0-9]+)?").unwrap();
    let mut out_blocks: Vec<String> = Vec::new();
    for (_c, b) in blocks {
        out_blocks.push(re_hp.replace_all(&b, |caps: &regex::Captures| format!("{} {}", &caps[1], hp)).to_string());
    }
    let mut inner_new_lines: Vec<String> = Vec::new();
    for blk in out_blocks {
        inner_new_lines.extend(blk.lines().map(|s| s.to_string()));
    }
    if other_lines.iter().any(|s| !s.trim().is_empty()) {
        inner_new_lines.extend(other_lines);
    }
    let new_inner = inner_new_lines.join(nl);
    format!("{}{}{}{}{}", &text[..(open + 1)], nl, new_inner, nl, &text[close..])
}

fn build_zone_fractal_from_preset(
    preset_text: &str,
    base_guid: &str,
    base_res: &str,
    v2_guid: Option<&str>,
    v2_res: Option<&str>,
    zone_count: usize,
    hp_zone: i32,
) -> String {
    let mut s = strip_preset_header(preset_text);
    s = replace_object_marker(&s, base_guid, base_res);
    if let (Some(g), Some(r)) = (v2_guid, v2_res) {
        s = replace_v2_model_marker(&s, g, r);
    }
    s = replace_gen_guid_markers(&s);
    s = ensure_fractalparts_zone_count(&s, zone_count);
    s = apply_zone_hp(&s, hp_zone);
    s
}

#[derive(Serialize)]
struct PrefabDstBuildResult {
    out_paths: Vec<String>,
}

#[tauri::command]
async fn prefabdst_build(
    app: tauri::AppHandle,
    preset_file: String,
    preset_text: String,
    zones: usize,
    hp_zone: i32,
    model_files: Vec<String>,
    save_folder: String,
    scr_override: Option<ScrDstScanResult>,
) -> Result<PrefabDstBuildResult, String> {
    let zones = zones.clamp(1, 26);
    let hp_zone = hp_zone.clamp(1, 9999);
    if model_files.is_empty() {
        return Err("No model files".into());
    }
    if save_folder.trim().is_empty() {
        return Err("No save folder".into());
    }
    let out_dir = PathBuf::from(save_folder.trim());
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let total = model_files.len();
    emit_prefabdst_log(&app, "info", format!("Preset: {}", preset_file), None, None);
    emit_prefabdst_log(&app, "info", format!("Zones: {} (hp={})", zones, hp_zone), None, None);

    let mut out_paths: Vec<String> = Vec::new();
    for (idx, xob_path) in model_files.iter().enumerate() {
        let cur = idx + 1;
        emit_prefabdst_log(&app, "info", format!("Reading meta for: {}", xob_path), Some(cur), Some(total));
        let xob_abs = PathBuf::from(xob_path);
        let (base_guid, base_res) = read_xob_object_field_from_meta(&xob_abs)?;

        // v2 model (.xob.meta) next to base file
        let mut v2_guid: Option<String> = None;
        let mut v2_res: Option<String> = None;
        if let Some(stem) = xob_abs.file_stem().and_then(|s| s.to_str()) {
            if let Some(parent) = xob_abs.parent() {
                let v2 = parent.join(format!("{}_V2_dst.xob", stem));
                if v2.is_file() {
                    if let Ok((g2, r2)) = read_xob_object_field_from_meta(&v2) {
                        v2_guid = Some(g2);
                        v2_res = Some(r2);
                    }
                }
            }
        }

        // Branch by preset generator
        let hdr = parse_preset_header(&preset_text);
        let gen_key = hdr.generator.trim().to_lowercase();
        emit_prefabdst_log(&app, "info", format!("Generator: {}", if gen_key.is_empty() { "(unknown)" } else { &gen_key }), Some(cur), Some(total));

        // For zone_fractal presets, auto-detect the number of zones from GeometryParam part tags (A-Z)
        // via the same scan used for debris/collider marker replacement.
        let mut full_scan: Option<FullDstScanResult> = None;
        let mut zones_for_this_file = zones;
        if gen_key != "template" {
            match prefabdst_scan_full_dst(xob_path.to_string()).await {
                Ok(scan) => {
                    let inferred = scan.zones.len().clamp(1, 26);
                    zones_for_this_file = inferred;
                    full_scan = Some(scan);
                    emit_prefabdst_log(
                        &app,
                        "info",
                        format!("Auto zones: {} (from GeometryParam tags)", inferred),
                        Some(cur),
                        Some(total)
                    );
                }
                Err(e) => {
                    emit_prefabdst_log(
                        &app,
                        "warn",
                        format!("Auto zones skipped (full dst scan failed): {}", e),
                        Some(cur),
                        Some(total)
                    );
                }
            }
        }

        emit_prefabdst_log(&app, "info", "Generating ET text...", Some(cur), Some(total));
        let mut et_text = if gen_key == "template" {
            // scr_destructible template rendering (auto-scan dst)
            render_scr_destructible_template(&preset_text, &base_guid, &base_res, &xob_abs, scr_override.as_ref())
        } else {
            // default to zone_fractal
            build_zone_fractal_from_preset(
                &preset_text,
                &base_guid,
                &base_res,
                v2_guid.as_deref(),
                v2_res.as_deref(),
                zones_for_this_file,
                hp_zone,
            )
        };

        // For zone_fractal presets, fill debris/collider markers using a scan that matches the UI tree.
        if gen_key != "template" {
            if et_text.contains("{{DEBRIS_ID-") || et_text.contains("{{COLLIDERS_ID-") {
                if let Some(scan) = full_scan.as_ref() {
                    et_text = replace_full_dst_markers(&et_text, scan);
                } else {
                    emit_prefabdst_log(
                        &app,
                        "warn",
                        "Full DST scan unavailable (markers not replaced)".to_string(),
                        Some(cur),
                        Some(total)
                    );
                }
            }
        }

        // Always normalize any remaining {gen vec3} marker to 0 0 0.
        et_text = replace_gen_vec3_markers(&et_text);

        let stem = xob_abs
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "Invalid xob file name".to_string())?;
        let out_path = out_dir.join(format!("{}_test_dst_prefab.et", stem));
        emit_prefabdst_log(&app, "info", format!("Writing: {}", out_path.to_string_lossy()), Some(cur), Some(total));
        fs::write(&out_path, et_text).map_err(|e| e.to_string())?;
        out_paths.push(out_path.to_string_lossy().to_string());
    }

    emit_prefabdst_log(&app, "info", format!("Done. Generated {} file(s).", out_paths.len()), None, None);
    Ok(PrefabDstBuildResult { out_paths })
}
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[derive(Serialize, Clone)]
struct TunnelInfo { url: String, pid: u32 }

const CLOUDFLARED_URL: &str = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe";

async fn is_pe_exe(path: &Path) -> bool {
    if let Ok(bytes) = tokio::fs::read(path).await {
        return bytes.len() >= 2 && bytes[0] == b'M' && bytes[1] == b'Z';
    }
    false
}

async fn ensure_cloudflared_binary(app: Option<&tauri::AppHandle>) -> Result<PathBuf, String> {
    let bin_dir = ensure_data_dir().join("bin");
    fs::create_dir_all(&bin_dir).map_err(|e| format!("Failed to create bin dir: {}", e))?;
    let exe_path = bin_dir.join("cloudflared.exe");

    if exe_path.is_file() {
        if let Ok(meta) = fs::metadata(&exe_path) {
            if meta.len() > 0 && is_pe_exe(&exe_path).await {
                return Ok(exe_path);
            }
        }
        let _ = tokio::fs::remove_file(&exe_path).await;
    }

    if let Some(app) = app {
        let _ = app.emit("updater://info", json!({ "message": "Downloading cloudflared..." }));
    }

    let client = reqwest::Client::builder()
        .user_agent("OwlTools-Cloudflared")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let resp = client
        .get(CLOUDFLARED_URL)
        .send()
        .await
        .map_err(|e| format!("Cloudflared download request failed: {}", e))?;
    let final_url = resp.url().to_string();
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Cloudflared download failed: HTTP {} url={}", status, final_url));
    }

    if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(cts) = ct.to_str() {
            let cts_l = cts.to_lowercase();
            if cts_l.contains("text/html") {
                return Err(format!(
                    "Cloudflared download returned unexpected content-type: {} url={} status={}",
                    cts, final_url, status
                ));
            }
        }
    }

    let tmp_path = bin_dir.join("cloudflared.download");
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| format!("Failed to create cloudflared file: {}", e))?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Cloudflared download stream error: {}", e))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Cloudflared write failed: {}", e))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
    }
    file.flush().await.map_err(|e| format!("Cloudflared flush failed: {}", e))?;

    // Basic validation: ensure Windows PE header.
    if !is_pe_exe(&tmp_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err("Cloudflared download did not look like a Windows executable".into());
    }

    // Move into place
    let _ = tokio::fs::remove_file(&exe_path).await;
    tokio::fs::rename(&tmp_path, &exe_path)
        .await
        .map_err(|e| format!("Failed to finalize cloudflared: {}", e))?;

    if downloaded == 0 {
        let _ = tokio::fs::remove_file(&exe_path).await;
        return Err("Cloudflared download produced empty file".into());
    }

    Ok(exe_path)
}

#[tauri::command]
async fn start_quick_tunnel_unique() -> Result<TunnelInfo, String> {
    let cloudflared = ensure_cloudflared_binary(None).await?;
    let mut cmd = TokioCommand::new(cloudflared);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd.arg("tunnel").arg("--no-autoupdate").arg("--loglevel").arg("info")
        .arg("--url").arg("http://127.0.0.1:8787")
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Failed to start cloudflared: {}", e))?;
    let pid = child.id().ok_or_else(|| "Failed to get cloudflared PID".to_string())?;

    let stdout = child.stdout.take().ok_or_else(|| "No stdout from cloudflared".to_string())?;
    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines_opt = child.stderr.take().map(|s| BufReader::new(s).lines());

    // Receive URL from readers
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    {
        let tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            while let Ok(Some(line)) = out_lines.next_line().await {
                if let Some(url) = line.split_whitespace().find(|&s| s.contains("trycloudflare.com") && s.starts_with("http")) {
                    let _ = tx.send(url.to_string());
                    break;
                }
            }
        });
    }
    if let Some(mut err_lines) = err_lines_opt.take() {
        let tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            while let Ok(Some(line)) = err_lines.next_line().await {
                if let Some(url) = line.split_whitespace().find(|&s| s.contains("trycloudflare.com") && s.starts_with("http")) {
                    let _ = tx.send(url.to_string());
                    break;
                }
            }
        });
    }

    let mut url: Option<String> = None;
    for _ in 0..80u32 { // ~8s
        tokio::select! {
            v = rx.recv() => { url = v; break; }
            _ = sleep(Duration::from_millis(100)) => {}
        }
    }
    let url = match url { Some(u) if !u.is_empty() => u, _ => {
        let _ = child.kill().await; return Err("Cloudflared did not provide a public URL in time".into());
    }};

    // store child keyed by pid so we can stop it later
    let map = CLOUD_CHILDREN.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone();
    if let Ok(mut g) = map.lock() { g.insert(pid, child); }

    Ok(TunnelInfo { url, pid })
}

#[tauri::command]
async fn stop_quick_tunnel(pid: u32) -> Result<(), String> {
    // Remove the child under the mutex, then drop the guard before awaiting
    let child_opt = {
        let map = CLOUD_CHILDREN.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
        let x = match map.lock() {
            Ok(mut g) => g.remove(&pid),
            Err(_) => None,
        };
        x
    };
    if let Some(mut child) = child_opt {
        child
            .kill()
            .await
            .map_err(|e| format!("Failed to kill cloudflared: {}", e))?;
        Ok(())
    } else {
        Err("Tunnel process not found".into())
    }
}

static CLOUD_CHILD: OnceCell<Arc<Mutex<Option<tokio::process::Child>>>> = OnceCell::new();
static CLOUD_CHILDREN: OnceCell<Arc<Mutex<HashMap<u32, tokio::process::Child>>>> = OnceCell::new();

#[tauri::command]
async fn start_quick_tunnel(state: tauri::State<'_, RemoteState>) -> Result<String, String> {
    let child_cell = CLOUD_CHILD.get_or_init(|| Arc::new(Mutex::new(None))).clone();
    // If we already have a URL, return it
    if let Ok(guard) = state.inner.lock() {
        if let Some(url) = guard.tunnel_url.clone() { return Ok(url); }
    }
    let already_running = child_cell
        .lock()
        .map(|cg| cg.as_ref().and_then(|c: &tokio::process::Child| c.id()).is_some())
        .unwrap_or(false);
    if already_running {
        for _ in 0..80u32 {
            if let Ok(g) = state.inner.lock() { if let Some(u) = g.tunnel_url.clone() { return Ok(u); } }
            sleep(Duration::from_millis(100)).await;
        }
    }

    let cloudflared = ensure_cloudflared_binary(Some(&state.app)).await?;
    let mut cmd = TokioCommand::new(cloudflared);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd.arg("tunnel").arg("--no-autoupdate").arg("--loglevel").arg("info")
        .arg("--url").arg("http://127.0.0.1:8787")
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Failed to start cloudflared: {}", e))?;
    let stdout = child.stdout.take().ok_or_else(|| "No stdout from cloudflared".to_string())?;
    let stderr = child.stderr.take();
    let state_clone = state.inner.clone();
    let app = state.app.clone();
    // Monitor stdout for public URL
    tauri::async_runtime::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if line.as_str().contains("trycloudflare.com") {
                if let Some(url) = line.split_whitespace().find(|&s| s.contains("trycloudflare.com") && s.starts_with("http")) {
                    let url = url.trim().to_string();
                    if !url.is_empty() {
                        if let Ok(mut g) = state_clone.lock() { g.tunnel_url = Some(url.clone()); }
                        let _ = app.emit("tunnel_url", url.clone());
                    }
                }
            }
        }
    });
    // Also monitor stderr, since cloudflared often logs there
    if let Some(stderr) = stderr {
        let state_clone = state.inner.clone();
        let app = state.app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.as_str().contains("trycloudflare.com") {
                    if let Some(url) = line.split_whitespace().find(|&s| s.contains("trycloudflare.com") && s.starts_with("http")) {
                        let url = url.trim().to_string();
                        if !url.is_empty() {
                            if let Ok(mut g) = state_clone.lock() { g.tunnel_url = Some(url.clone()); }
                            let _ = app.emit("tunnel_url", url.clone());
                        }
                    }
                }
            }
        });
    }
    // store child
    if let Ok(mut cg) = child_cell.lock() { *cg = Some(child); }
    // wait for first URL up to ~8s
    for _ in 0..80u32 {
        if let Ok(g) = state.inner.lock() { if let Some(u) = g.tunnel_url.clone() { return Ok(u); } }
        sleep(Duration::from_millis(100)).await;
    }
    Err("Cloudflared did not provide a public URL in time".into())
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn write_text_file(path: String, content: String) -> Result<(), String> {
    fs::write(&path, content).map_err(|e| e.to_string())
}

fn owltools_data_dir() -> PathBuf {
    document_dir()
        .or_else(|| home_dir())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OwlTools")
}

fn ensure_data_dir() -> PathBuf {
    let dir = owltools_data_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("Failed to create data dir {:?}: {}", dir, err);
    }
    dir
}

fn ensure_updates_dir() -> PathBuf {
    let dir = ensure_data_dir().join("updates");
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("Failed to create updates dir {:?}: {}", dir, err);
    }
    dir
}

#[derive(Serialize, Clone)]
struct UpdaterProgressPayload {
    version: String,
    percent: Option<f64>,
    bytes: Option<u64>,
    total: Option<u64>,
    message: Option<String>,
}

#[derive(Serialize, Clone)]
struct UpdaterDownloadResult {
    local_path: String,
}

#[tauri::command]
async fn updater_download_msi(app: tauri::AppHandle, version: String, url: String, sha256: String) -> Result<UpdaterDownloadResult, String> {
    let version = version.trim().to_string();
    if version.is_empty() {
        return Err("Missing version".into());
    }
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("Missing url".into());
    }
    let expected = sha256.trim().to_lowercase();
    if expected.is_empty() {
        return Err("Missing sha256".into());
    }

    let updates_dir = ensure_updates_dir();
    let target_dir = updates_dir.join(&version);
    fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    let msi_path = target_dir.join(format!("OwlTools_{}.msi", version));

    let _ = app.emit(
        "updater://download_progress",
        UpdaterProgressPayload {
            version: version.clone(),
            percent: Some(0.0),
            bytes: Some(0),
            total: None,
            message: Some("Starting download...".into()),
        },
    );

    let client = reqwest::Client::builder()
        .user_agent("OwlTools-Updater")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
    let resp = client.get(&url).send().await.map_err(|e| format!("Request failed: {}", e))?;
    let final_url = resp.url().to_string();
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Download failed: HTTP {} url={}", status, final_url));
    }

    if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(cts) = ct.to_str() {
            let cts_l = cts.to_lowercase();
            if cts_l.contains("text/html") || cts_l.contains("text/plain") {
                let _ = app.emit(
                    "updater://info",
                    json!({ "message": format!("Updater download got unexpected content-type. status={} content-type={} url={}", status, cts, final_url) }),
                );
                return Err(format!("Download returned unexpected content-type: {} url={} status={}", cts, final_url, status));
            }
        }
    }
    let total = resp.content_length();

    let mut file = tokio::fs::File::create(&msi_path).await.map_err(|e| format!("Failed to create file: {}", e))?;
    let mut stream = resp.bytes_stream();
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        file.write_all(&chunk).await.map_err(|e| format!("Write failed: {}", e))?;
        hasher.update(&chunk);
        downloaded = downloaded.saturating_add(chunk.len() as u64);

        let percent = total.map(|t| if t > 0 { (downloaded as f64) * 100.0 / (t as f64) } else { 0.0 });
        let _ = app.emit(
            "updater://download_progress",
            UpdaterProgressPayload {
                version: version.clone(),
                percent,
                bytes: Some(downloaded),
                total,
                message: None,
            },
        );
    }

    file.flush().await.map_err(|e| format!("Flush failed: {}", e))?;

    let actual = hex::encode(hasher.finalize());
    if actual.to_lowercase() != expected {
        let _ = tokio::fs::remove_file(&msi_path).await;
        return Err(format!("SHA256 mismatch. expected={} actual={}", expected, actual));
    }

    let _ = app.emit(
        "updater://download_progress",
        UpdaterProgressPayload {
            version: version.clone(),
            percent: Some(100.0),
            bytes: Some(downloaded),
            total,
            message: Some("Download verified".into()),
        },
    );

    Ok(UpdaterDownloadResult {
        local_path: msi_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn updater_install_msi(app: tauri::AppHandle, version: String) -> Result<(), String> {
    let version = version.trim().to_string();
    if version.is_empty() {
        return Err("Missing version".into());
    }

    let updates_dir = ensure_updates_dir();
    let msi_path = updates_dir
        .join(&version)
        .join(format!("OwlTools_{}.msi", version));
    if !msi_path.is_file() {
        return Err(format!("MSI not found for version: {}", version));
    }

    let _ = app.emit(
        "updater://info",
        json!({ "message": "Launching MSI installer..." }),
    );

    let msi_s = msi_path.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        // Updating an MSI installed under Program Files typically requires elevation.
        // Use UAC prompt (RunAs). Do not use /qn.
        let msi_ps = msi_s.replace('"', "\"\"").replace('`', "``");
        let cmdline = format!(
            "Start-Process msiexec -Verb RunAs -ArgumentList @('/i','{}')",
            msi_ps
        );
        let mut cmd = TokioCommand::new("powershell");
        cmd.arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(cmdline)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn()
            .map_err(|e| format!("Failed to launch elevated installer: {}", e))?;
    }
    #[cfg(not(windows))]
    {
        // Fallback: just run msiexec directly where applicable.
        let mut cmd = TokioCommand::new("msiexec");
        cmd.arg("/i")
            .arg(msi_s)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn().map_err(|e| format!("Failed to launch msiexec: {}", e))?;
    }

    // Close the app so the installer can replace files.
    app.exit(0);
    Ok(())
}

fn prefab_index_path() -> PathBuf {
    ensure_data_dir().join("AutoSocket_PrefabIndex.json")
}

fn settings_path() -> PathBuf {
    ensure_data_dir().join("AutoSocket_Settings.json")
}

fn presets_path() -> PathBuf {
    ensure_data_dir().join("AutoSocket_Presets.json")
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct AutoSettings {
    svn_root: Option<String>,
    save_dir: Option<String>,
    extra_dirs: Option<Vec<String>>,
    blender_path: Option<String>,
    ebt_addons_dir: Option<String>,
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct AutoPreset {
    name: String,
    save_dir: Option<String>,
    extra_dirs: Option<Vec<String>>,
    blender_path: Option<String>,
}

fn load_presets() -> Vec<AutoPreset> {
    let path = presets_path();
    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(p) = serde_json::from_str::<Vec<AutoPreset>>(&text) {
            return p;
        }
    }
    Vec::new()
}

fn save_presets(presets: &Vec<AutoPreset>) -> Result<(), String> {
    let path = presets_path();
    fs::write(&path, serde_json::to_string_pretty(presets).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn load_settings() -> AutoSettings {
    let path = settings_path();
    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(s) = serde_json::from_str::<AutoSettings>(&text) {
            return s;
        }
    }
    AutoSettings::default()
}

fn save_settings(settings: &AutoSettings) -> Result<(), String> {
    let path = settings_path();
    fs::write(&path, serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_autosocket_settings() -> Result<AutoSettings, String> {
    Ok(load_settings())
}

#[tauri::command]
fn get_autosocket_presets() -> Result<Vec<AutoPreset>, String> {
    Ok(load_presets())
}

#[tauri::command]
fn save_autosocket_presets(presets: Vec<AutoPreset>) -> Result<(), String> {
    let mut cleaned: Vec<AutoPreset> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for mut p in presets {
        p.name = p.name.trim().to_string();
        if p.name.is_empty() {
            continue;
        }
        if !seen.insert(p.name.to_lowercase()) {
            continue;
        }
        if let Some(sd) = p.save_dir.as_ref() {
            if sd.trim().is_empty() {
                p.save_dir = None;
            }
        }
        if let Some(bp) = p.blender_path.as_ref() {
            if bp.trim().is_empty() {
                p.blender_path = None;
            }
        }
        if let Some(ed) = p.extra_dirs.as_ref() {
            let arr: Vec<String> = ed
                .iter()
                .filter_map(|x| {
                    let t = x.trim();
                    if t.is_empty() { None } else { Some(t.to_string()) }
                })
                .collect();
            p.extra_dirs = Some(arr);
        }
        cleaned.push(p);
    }
    save_presets(&cleaned)
}

#[tauri::command]
fn remember_save_dir(path: Option<String>) -> Result<(), String> {
    let mut settings = load_settings();
    settings.save_dir = path.filter(|p| !p.is_empty());
    save_settings(&settings)
}

#[tauri::command]
fn remember_extra_dirs(extra_dirs: Vec<String>) -> Result<(), String> {
    let mut cleaned: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for p in extra_dirs {
        let pb = PathBuf::from(p);
        if !pb.is_dir() {
            continue;
        }
        let norm = pb
            .canonicalize()
            .unwrap_or(pb)
            .to_string_lossy()
            .to_string();
        if seen.insert(norm.clone()) {
            cleaned.push(norm);
        }
    }
    let mut settings = load_settings();
    settings.extra_dirs = Some(cleaned);
    save_settings(&settings)
}

#[tauri::command]
fn remember_blender_path(path: Option<String>) -> Result<(), String> {
    let mut settings = load_settings();
    settings.blender_path = path.filter(|p| !p.is_empty());
    save_settings(&settings)
}

#[tauri::command]
fn remember_ebt_addons_dir(path: Option<String>) -> Result<(), String> {
    let mut settings = load_settings();
    settings.ebt_addons_dir = path.filter(|p| !p.is_empty());
    save_settings(&settings)
}

#[tauri::command]
fn mqa_report_from_xob(
    xob_path: String,
    workbench_port: Option<u16>,
    asset_type: Option<String>,
) -> Result<JsonValue, String> {
    let xob_abs = PathBuf::from(&xob_path);
    if !xob_abs.is_file() {
        return Err("Invalid xob path".into());
    }
    let fbx_abs = xob_abs.with_extension("fbx");
    if !fbx_abs.is_file() {
        return Err(format!(
            "FBX not found next to xob: {}",
            fbx_abs.to_string_lossy()
        ));
    }

    let settings = load_settings();
    let blender = settings
        .blender_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(resolve_blender_path)
        .ok_or_else(|| "Blender not configured".to_string())?;

    let addons_dir = settings
        .ebt_addons_dir
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .ok_or_else(|| "EBT addons directory not configured".to_string())?;

    let wb_port: u16 = workbench_port.unwrap_or(5700);

    let asset_type_norm = asset_type
        .unwrap_or_else(|| "GENERIC".to_string())
        .trim()
        .to_uppercase();
    let asset_type_norm = match asset_type_norm.as_str() {
        "GENERIC" | "BUILDINGS" | "VEHICLES" | "WEAPONS" => asset_type_norm,
        _ => "GENERIC".to_string(),
    };

    let py = format!(
        r#"import sys, pathlib, json
import bpy
import addon_utils

debug = {{}}
errors = []

addons_dir = pathlib.Path(r'''{}''')
if not addons_dir.exists():
    raise RuntimeError('EBT addons dir does not exist: ' + str(addons_dir))
sys.path.insert(0, str(addons_dir))

try:
    addon_utils.enable('EnfusionBlenderTools', default_set=False, persistent=False)
    debug['addon_enabled'] = True
except Exception:
    debug['addon_enabled'] = False

try:
    import EnfusionBlenderTools
    debug['ebt_imported'] = True
except Exception as e:
    debug['ebt_imported'] = False
    errors.append('import EnfusionBlenderTools failed: ' + str(e))

try:
    import EnfusionBlenderTools.modelqa as modelqa
    try:
        modelqa.register()
        debug['modelqa_registered'] = True
    except Exception as e:
        msg = str(e)
        if 'already registered' in msg or 'already registered as a subclass' in msg:
            debug['modelqa_registered'] = True
            debug['modelqa_register_already'] = True
        else:
            debug['modelqa_registered'] = False
            errors.append('modelqa.register failed: ' + msg)
except Exception as e:
    debug['modelqa_registered'] = False
    errors.append('import modelqa failed: ' + str(e))

try:
    from EnfusionBlenderTools.workbench import Workbench
    from EnfusionBlenderTools.core.fbx import fbx_io
    debug['ebt_modules_ok'] = True
except Exception as e:
    debug['ebt_modules_ok'] = False
    errors.append('import Workbench/fbx_io failed: ' + str(e))
    raise

Workbench.init(client_id='OwlTools', port={})
fbx = pathlib.Path(r'''{}''')
fbx_io.import_fbx(fbx)

op_exists = False
try:
    if hasattr(bpy.ops, 'ebt') and ('mqa_report_conventions' in dir(bpy.ops.ebt)):
        op_exists = True
except Exception:
    op_exists = False
debug['mqa_operator_exists'] = op_exists

if op_exists:
    try:
        bpy.ops.ebt.mqa_report_conventions(asset_type=r'''{}''')
        debug['mqa_operator_ran'] = True
    except Exception as e:
        debug['mqa_operator_ran'] = False
        errors.append('mqa_report_conventions failed: ' + str(e))
else:
    debug['mqa_operator_ran'] = False
    errors.append('mqa_report_conventions operator not found')

scene = bpy.context.scene
items = []
try:
    if not hasattr(scene, 'ebt_report_messages'):
        raise RuntimeError('scene.ebt_report_messages not found (modelqa not registered?)')
    msgs = scene.ebt_report_messages
    for m in msgs:
        objs = []
        try:
            for o in m.objs:
                objs.append(getattr(o, 'name', ''))
        except Exception:
            pass
        items.append(dict(
            category=getattr(m, 'category', ''),
            message=getattr(m, 'message', ''),
            count=len(objs),
            objects=objs,
        ))
except Exception as e:
    items = []
    errors.append('collect messages failed: ' + str(e))

payload = dict(fbx=str(fbx), count=len(items), items=items, debug=debug, errors=errors)
print('OWLTOOLS_MQA_JSON=' + json.dumps(payload, ensure_ascii=False))
"#,
        addons_dir.to_string_lossy(),
        wb_port,
        fbx_abs.to_string_lossy()
        , asset_type_norm
    );

    let out = std::process::Command::new(blender)
        .args(["--background", "--factory-startup", "--python-expr", &py])
        .output()
        .map_err(|e| format!("Failed to run Blender: {}", e))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let mut json_line: Option<String> = None;
    for line in stdout.lines().rev() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("OWLTOOLS_MQA_JSON=") {
            json_line = Some(rest.to_string());
            break;
        }
    }
    let json_line = json_line.ok_or_else(|| {
        if stderr.trim().is_empty() {
            "Failed to capture MQA report from Blender".to_string()
        } else {
            format!("Failed to capture MQA report from Blender: {}", stderr.trim())
        }
    })?;
    serde_json::from_str::<JsonValue>(&json_line).map_err(|e| e.to_string())
}

 #[tauri::command]
 fn open_fbx_in_blender(xob_path: String, workbench_port: Option<u16>) -> Result<(), String> {
     let xob_abs = PathBuf::from(&xob_path);
     if !xob_abs.is_file() {
         return Err("Invalid xob path".into());
     }
     let fbx_abs = xob_abs.with_extension("fbx");
     if !fbx_abs.is_file() {
         return Err(format!(
             "FBX not found next to xob: {}",
             fbx_abs.to_string_lossy()
         ));
     }

     let settings = load_settings();
     let blender = settings
         .blender_path
         .as_deref()
         .map(PathBuf::from)
         .filter(|p| p.is_file())
         .or_else(resolve_blender_path)
         .ok_or_else(|| "Blender not configured".to_string())?;

     let addons_dir = settings
         .ebt_addons_dir
         .as_deref()
         .map(PathBuf::from)
         .filter(|p| p.is_dir())
         .ok_or_else(|| "EBT addons directory not configured".to_string())?;

     let wb_port: u16 = workbench_port.unwrap_or(5700);

     let py = format!(
         r#"import sys, pathlib
import bpy
import addon_utils

addons_dir = pathlib.Path(r'''{}''')
if not addons_dir.exists():
    raise RuntimeError('EBT addons dir does not exist: ' + str(addons_dir))
sys.path.insert(0, str(addons_dir))

try:
    addon_utils.enable('EnfusionBlenderTools', default_set=True, persistent=True)
    try:
        bpy.ops.wm.save_userpref()
    except Exception:
        pass
except Exception:
    pass

try:
    from EnfusionBlenderTools.workbench import Workbench
    from EnfusionBlenderTools.core.fbx import fbx_io
except Exception as e:
    raise RuntimeError('Failed to import EnfusionBlenderTools modules: ' + str(e))

try:
    Workbench.init(client_id='OwlTools', port={})
except Exception as e:
    raise RuntimeError('Workbench.init failed: ' + str(e))

fbx = pathlib.Path(r'''{}''')
fbx_io.import_fbx(fbx)
try:
    bpy.ops.ebt.mqa_report_conventions()
except Exception:
    pass
"#,
         addons_dir.to_string_lossy(),
         wb_port,
         fbx_abs.to_string_lossy()
     );

     std::process::Command::new(blender)
         .args(["--factory-startup", "--python-expr", &py])
         .spawn()
         .map_err(|e| format!("Failed to launch Blender: {}", e))?;
     Ok(())
 }

fn cached_prefab_status() -> PrefabCacheStatus {
    let path = prefab_index_path();
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(value) = serde_json::from_str::<JsonValue>(&text) {
            let generated = value
                .get("generated")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let svn_root = value
                .get("svn_root")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let prefab_count = value
                .get("prefabs")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .or_else(|| value.get("name_index").and_then(|v| v.as_object()).map(|o| o.len()))
                .unwrap_or(0);
            return PrefabCacheStatus {
                has_cache: true,
                cache_path: Some(path.to_string_lossy().to_string()),
                svn_root,
                generated,
                prefab_count,
            };
        }
    }
    PrefabCacheStatus::default()
}

fn detect_svn_candidates() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        bases.push(home.clone());
        for sub in ["Documents", "Desktop", "Downloads", "Projects", "Project", "Work", "Workspace"] {
            let cand = home.join(sub);
            if cand.is_dir() {
                bases.push(cand);
            }
        }
    }
    if let Some(docs) = document_dir() {
        bases.push(docs);
    }
    if let Ok(env) = std::env::var("SVN_ROOT") {
        let pb = PathBuf::from(env);
        if pb.is_dir() {
            bases.push(pb);
        }
    }
    for drv in 'A'..='Z' {
        let root = format!("{}:\\", drv);
        let pb = PathBuf::from(&root);
        if pb.is_dir() {
            bases.push(pb);
        }
    }
    bases
}

fn find_svn_in_base(base: &Path, max_entries: usize) -> Option<PathBuf> {
    if !base.is_dir() {
        return None;
    }
    let mut seen = 0usize;
    for entry in WalkDir::new(base)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.eq_ignore_ascii_case("svn") {
            return Some(entry.into_path());
        }
        seen += 1;
        if seen >= max_entries {
            break;
        }
    }
    None
}

fn auto_detect_svn_blocking() -> Option<PathBuf> {
    let settings = load_settings();
    if let Some(ref stored) = settings.svn_root {
        let pb = PathBuf::from(stored);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    let cache = cached_prefab_status();
    if let Some(ref cached) = cache.svn_root {
        let pb = PathBuf::from(cached);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    let mut visited = HashSet::new();
    for base in detect_svn_candidates() {
        let canonical = base.clone();
        if !visited.insert(canonical.clone()) {
            continue;
        }
        if let Some(found) = find_svn_in_base(&base, 10_000) {
            return Some(found);
        }
    }
    None
}

fn read_meta_name_field(meta_path: &Path) -> Option<String> {
    if !meta_path.is_file() {
        return None;
    }
    fs::read_to_string(meta_path).ok().and_then(|text| {
        let needle = "Name \"";
        let idx = text.find(needle)?;
        let rest = &text[idx + needle.len()..];
        let end_idx = rest.find('"')?;
        Some(rest[..end_idx].trim().to_string())
    })
}

fn extract_guid(name_value: &str) -> Option<String> {
    let trimmed = name_value.trim_start();
    if let Some(stripped) = trimmed.strip_prefix('{') {
        if let Some(end_idx) = stripped.find('}') {
            let guid = &stripped[..end_idx];
            if guid.len() == 16 && guid.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(guid.to_uppercase());
            }
        }
    }
    None
}

fn meta_mtime_seconds(meta_path: &Path) -> Option<f64> {
    let meta = fs::metadata(meta_path).ok()?;
    let modified = meta.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_secs_f64())
}

fn update_prefab_cache_with_new_meta(et_path: &Path, meta_path: &Path, name_value: &str) -> Result<(), String> {
    let et_str = et_path.to_string_lossy().to_string();
    let meta_str = meta_path.to_string_lossy().to_string();
    let key = et_path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| et_str.to_lowercase());

    let (svn_root, mut name_index, mut guid_index, mut et_path_index, mut guid_path_index, mut meta_mtime) =
        match load_existing_prefab_cache() {
            Some((prev_root, n, g, etp, gpp, mt)) => (prev_root, n, g, etp, gpp, mt),
            None => (String::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        };

    name_index.insert(key.clone(), name_value.to_string());
    et_path_index.insert(key, et_str.clone());
    if let Some(guid) = extract_guid(name_value) {
        guid_index.insert(guid.clone(), name_value.to_string());
        guid_path_index.insert(guid, et_str);
    }
    if let Some(m) = meta_mtime_seconds(meta_path) {
        meta_mtime.insert(meta_str, m);
    }

    let cache_path = prefab_index_path();
    let mut payload = json!({
        "version": 1,
        "generated": Utc::now().to_rfc3339(),
        "svn_root": svn_root,
        "name_index": name_index,
        "guid_index": guid_index,
        "et_path_index": et_path_index,
        "guid_path_index": guid_path_index,
    });
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("meta_mtime".to_string(), json!(meta_mtime));
    }
    fs::write(
        &cache_path,
        serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn load_existing_prefab_cache() -> Option<(String, BTreeMap<String, String>, BTreeMap<String, String>, BTreeMap<String, String>, BTreeMap<String, String>, BTreeMap<String, f64>)> {
    let fp = prefab_index_path();
    let txt = fs::read_to_string(fp).ok()?;
    let v: JsonValue = serde_json::from_str(&txt).ok()?;
    let svn_root = v.get("svn_root").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut name_index: BTreeMap<String, String> = BTreeMap::new();
    let mut guid_index: BTreeMap<String, String> = BTreeMap::new();
    let mut et_path_index: BTreeMap<String, String> = BTreeMap::new();
    let mut guid_path_index: BTreeMap<String, String> = BTreeMap::new();
    let mut meta_mtime: BTreeMap<String, f64> = BTreeMap::new();

    if let Some(obj) = v.get("name_index").and_then(|x| x.as_object()) {
        for (k, vv) in obj {
            if let Some(s) = vv.as_str() {
                name_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    if let Some(obj) = v.get("guid_index").and_then(|x| x.as_object()) {
        for (k, vv) in obj {
            if let Some(s) = vv.as_str() {
                guid_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    if let Some(obj) = v.get("et_path_index").and_then(|x| x.as_object()) {
        for (k, vv) in obj {
            if let Some(s) = vv.as_str() {
                et_path_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    if let Some(obj) = v.get("guid_path_index").and_then(|x| x.as_object()) {
        for (k, vv) in obj {
            if let Some(s) = vv.as_str() {
                guid_path_index.insert(k.to_string(), s.to_string());
            }
        }
    }
    if let Some(obj) = v.get("meta_mtime").and_then(|x| x.as_object()) {
        for (k, vv) in obj {
            if let Some(n) = vv.as_f64() {
                meta_mtime.insert(k.to_string(), n);
            }
        }
    }
    Some((svn_root, name_index, guid_index, et_path_index, guid_path_index, meta_mtime))
}

fn build_prefab_index(
    svn_root: &Path,
    verbose: bool,
    mut on_log: impl FnMut(&str, String, Option<(usize, usize)>),
) -> Result<(usize, PathBuf), String> {
    if !svn_root.is_dir() {
        return Err("SVN root is not a directory".into());
    }
    let canonical_root = svn_root
        .canonicalize()
        .unwrap_or_else(|_| svn_root.to_path_buf());

    on_log(
        "info",
        format!("SVN root: {}", canonical_root.to_string_lossy()),
        None,
    );
    on_log("info", "Scanning for .et.meta files...".to_string(), None);

    // Load previous cache for incremental update
    let (mut name_index, mut guid_index, mut et_path_index, mut guid_path_index, mut meta_mtime) =
        match load_existing_prefab_cache() {
            Some((prev_root, n, g, etp, gpp, mt)) => {
                if !prev_root.is_empty() && prev_root != canonical_root.to_string_lossy() {
                    on_log("info", "Existing cache svn_root differs; rebuilding indices".to_string(), None);
                    (BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new())
                } else {
                    on_log(
                        "info",
                        format!("Incremental scan: loaded cache (entries={})", n.len()),
                        None,
                    );
                    (n, g, etp, gpp, mt)
                }
            }
            None => (BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        };

    // Track current files to remove deletions
    let mut present_keys: HashSet<String> = HashSet::new();
    let mut present_et_paths: HashSet<String> = HashSet::new();
    let mut touched = 0usize;
    let mut updated = 0usize;

    const EXCLUDE_DIRS: [&str; 10] = [
        ".svn", "node_modules", ".git", ".idea", ".vscode", "Library", "Temp", "Logs", "obj", "bin",
    ];

    let walker = WalkDir::new(&canonical_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if !e.file_type().is_dir() {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if EXCLUDE_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d)) {
                return false;
            }
            true
        });

    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !name.to_lowercase().ends_with(".et.meta") {
            continue;
        }
        touched += 1;

        // Convert meta path -> et path
        let meta_path = entry.into_path();
        let meta_str = meta_path.to_string_lossy().to_string();
        let et_str = meta_str.trim_end_matches(".meta").to_string();
        let et_path = PathBuf::from(&et_str);

        let key = et_path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| et_str.to_lowercase());

        present_keys.insert(key.clone());
        present_et_paths.insert(et_str.clone());

        let mtime = meta_mtime_seconds(&meta_path).unwrap_or(0.0);
        let prev_m = meta_mtime.get(&meta_str).copied().unwrap_or(-1.0);
        if (mtime - prev_m).abs() < f64::EPSILON {
            // unchanged
            if !et_path_index.contains_key(&key) {
                et_path_index.insert(key.clone(), et_str.clone());
            }
            continue;
        }

        // Changed/new: parse and update
        let rel_path = et_path
            .strip_prefix(&canonical_root)
            .unwrap_or(&et_path)
            .to_string_lossy()
            .replace('\\', "/");
        let name_value = read_meta_name_field(&meta_path).unwrap_or_else(|| rel_path.clone());
        name_index.insert(key.clone(), name_value.clone());
        et_path_index.insert(key.clone(), et_str.clone());
        if let Some(guid) = extract_guid(&name_value) {
            guid_index.insert(guid.clone(), name_value.clone());
            guid_path_index.insert(guid.clone(), et_str.clone());
        }
        meta_mtime.insert(meta_str.clone(), mtime);
        updated += 1;

        if verbose {
            on_log("debug", format!("Updated {} -> {}", key, name_value), None);
        } else if updated == 1 || updated % 200 == 0 {
            on_log("info", format!("Indexing changes... {}", updated), None);
        }

        // Keep progress indeterminate (faster: no pre-count)
        if touched == 1 || touched % 1000 == 0 {
            on_log("info", format!("Scanning... {} meta", touched), Some((touched, touched + 1)));
        }
    }

    // Remove deleted keys
    let before = name_index.len();
    name_index.retain(|k, _| present_keys.contains(k));
    et_path_index.retain(|k, _| present_keys.contains(k));
    let removed_keys = before.saturating_sub(name_index.len());

    // Remove guid entries pointing to deleted paths
    guid_path_index.retain(|_, p| present_et_paths.contains(p));
    guid_index.retain(|g, _| guid_path_index.contains_key(g));

    // Remove stale meta_mtime entries
    meta_mtime.retain(|mp, _| {
        // only keep if meta still exists
        PathBuf::from(mp).is_file()
    });

    on_log(
        "info",
        format!("Scan summary: meta_seen={}, updated={}, removed_keys={}", touched, updated, removed_keys),
        None,
    );

    let cache_path = prefab_index_path();
    on_log(
        "info",
        format!("Writing cache: {}", cache_path.to_string_lossy()),
        None,
    );
    let mut payload = json!({
        "version": 1,
        "generated": Utc::now().to_rfc3339(),
        "svn_root": canonical_root.to_string_lossy(),
        "name_index": name_index,
        "guid_index": guid_index,
        "et_path_index": et_path_index,
        "guid_path_index": guid_path_index,
    });
    if !meta_mtime.is_empty() {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("meta_mtime".to_string(), json!(meta_mtime));
        }
    }
    fs::write(
        &cache_path,
        serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    on_log(
        "info",
        format!("Scan complete. Indexed {} prefabs", payload
            .get("name_index")
            .and_then(|v| v.as_object())
            .map(|o| o.len())
            .unwrap_or(0)),
        None,
    );
    Ok((name_index.len(), cache_path))
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct PrefabCacheStatus {
    has_cache: bool,
    cache_path: Option<String>,
    svn_root: Option<String>,
    generated: Option<String>,
    prefab_count: usize,
}

#[derive(Serialize)]
struct PrefabScanResult {
    total_entries: usize,
    cache_path: String,
}

#[tauri::command]
fn remember_svn_root(path: Option<String>) -> Result<(), String> {
    let mut settings = load_settings();
    settings.svn_root = path.filter(|p| !p.is_empty());
    save_settings(&settings)
}

#[tauri::command]
fn get_prefab_cache_status() -> Result<PrefabCacheStatus, String> {
    Ok(cached_prefab_status())
}

#[tauri::command]
async fn auto_detect_svn_root() -> Result<Option<String>, String> {
    let detected = tauri::async_runtime::spawn_blocking(|| auto_detect_svn_blocking())
        .await
        .map_err(|e| e.to_string())?;
    if let Some(path) = detected {
        if let Err(err) = remember_svn_root(Some(path.to_string_lossy().to_string())) {
            eprintln!("Failed to remember svn root: {}", err);
        }
        Ok(Some(path.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

#[tauri::command]
async fn scan_prefab_index(app: tauri::AppHandle, svn_root: String, verbose: Option<bool>) -> Result<PrefabScanResult, String> {
    let path = PathBuf::from(&svn_root);
    if !path.is_dir() {
        let msg = format!("SVN root does not exist or is not a directory: {}", svn_root);
        emit_scan_log(&app, "error", msg.clone(), None, None);
        return Err(msg);
    }
    let path_for_scan = path.clone();
    let app_for_scan = app.clone();
    let verbose = verbose.unwrap_or(false);
    emit_scan_log(&app, "info", "Starting prefab scan...", Some(0), None);
    let (total, cache_path) =
        tauri::async_runtime::spawn_blocking(move || {
            let mut last_progress: Option<(usize, usize)> = None;
            build_prefab_index(&path_for_scan, verbose, |level, msg, prog| {
                if let Some((c, t)) = prog {
                    last_progress = Some((c, t));
                }
                let (cur, tot) = match last_progress {
                    Some((c, t)) => (Some(c), Some(t)),
                    None => (None, None),
                };
                emit_scan_log(&app_for_scan, level, msg, cur, tot);
            })
        })
            .await
            .map_err(|e| e.to_string())??;
    remember_svn_root(Some(path.to_string_lossy().to_string())).ok();
    emit_scan_log(&app, "info", "Prefab scan finished", None, None);
    Ok(PrefabScanResult {
        total_entries: total,
        cache_path: cache_path.to_string_lossy().to_string(),
    })
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct MeshItem { name: String, status: String }

#[derive(Clone)]
struct RemoteState {
    app: tauri::AppHandle,
    inner: Arc<Mutex<RemoteData>>,
    tx: broadcast::Sender<String>,
}

#[derive(Default, Clone, Serialize, Deserialize, Debug)]
struct RemoteData {
    file_path: Option<String>,
    items: Vec<MeshItem>,
    tunnel_url: Option<String>,
    realtime: bool,
}

#[tauri::command]
fn set_mesh_state(state: tauri::State<RemoteState>, items: Vec<MeshItem>, file_path: Option<String>) -> Result<(), String> {
    if let Ok(mut guard) = state.inner.lock() {
        guard.items = items;
        guard.file_path = file_path;
    }
    broadcast_state(&state);
    Ok(())
}

#[tauri::command]
fn set_realtime_mode(state: tauri::State<RemoteState>, realtime: bool) -> Result<(), String> {
    if let Ok(mut guard) = state.inner.lock() {
        guard.realtime = realtime;
    }
    Ok(())
}

fn write_u32_le(stream: &mut TcpStream, v: u32) -> std::io::Result<()> {
    let b = v.to_le_bytes();
    stream.write_all(&b)
}

fn write_lp_str(stream: &mut TcpStream, s: &str) -> std::io::Result<()> {
    let b = s.as_bytes();
    write_u32_le(stream, b.len() as u32)?;
    stream.write_all(b)
}

fn read_i32_le(stream: &mut TcpStream) -> std::io::Result<i32> {
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_lp_str(stream: &mut TcpStream) -> std::io::Result<String> {
    let n = read_i32_le(stream)?;
    let n = if n < 0 { 0 } else { n as usize };
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

#[tauri::command]
fn wb_call(func_name: String, params: JsonValue, ip: Option<String>, port: Option<u16>, client_id: Option<String>) -> Result<JsonValue, String> {
    let ip = ip.unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.unwrap_or(5700);
    let client_id = client_id.unwrap_or_else(|| "Tauri-GUI".to_string());

    let mut req_obj = match params {
        JsonValue::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    req_obj.insert("APIFunc".to_string(), JsonValue::String(func_name));
    let req_json = JsonValue::Object(req_obj);
    let req_str = serde_json::to_string(&req_json).map_err(|e| e.to_string())?;

    let addr = format!("{}:{}", ip, port);
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("TCP_ERROR: {}", e))?;
    stream
        .set_nonblocking(false)
        .map_err(|e| format!("TCP_ERROR: {}", e))?;

    write_u32_le(&mut stream, 1).map_err(|e| e.to_string())?;
    write_lp_str(&mut stream, &client_id).map_err(|e| e.to_string())?;
    write_lp_str(&mut stream, "JsonRPC").map_err(|e| e.to_string())?;
    write_lp_str(&mut stream, &req_str).map_err(|e| e.to_string())?;

    let err = read_lp_str(&mut stream).map_err(|e| e.to_string())?;
    let payload = read_lp_str(&mut stream).map_err(|e| e.to_string())?;

    let mut out_obj = match serde_json::from_str::<JsonValue>(&payload).unwrap_or(JsonValue::Null) {
        JsonValue::Object(map) => map,
        JsonValue::Null => serde_json::Map::new(),
        v => {
            let mut m = serde_json::Map::new();
            m.insert("raw".to_string(), v);
            m
        }
    };
    out_obj.insert("ErrorCode".to_string(), JsonValue::String(err));
    Ok(JsonValue::Object(out_obj))
}

async fn get_mesh(AxumState(state): AxumState<RemoteState>) -> Json<RemoteData> {
    let data = state.inner.lock().map(|g| g.clone()).unwrap_or_default();
    Json(data)
}

fn broadcast_state(state: &RemoteState) {
    if let Ok(g) = state.inner.lock() {
        if let Ok(s) = serde_json::to_string(&*g) { let _ = state.tx.send(s); }
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct ToggleReq { name: String, status: Option<String> }

async fn post_toggle(AxumState(state): AxumState<RemoteState>, Json(req): Json<ToggleReq>) -> Json<JsonValue> {
    // compute next status and update in-memory items for remote UI
    let mut new_status: Option<String> = None;
    if let Ok(mut g) = state.inner.lock() {
        let desired = req.status.clone();
        let mut found = false;
        for it in &mut g.items {
            if it.name == req.name {
                let next = desired.clone().unwrap_or_else(|| if it.status == "Open" { "Hide".to_string() } else { "Open".to_string() });
                it.status = next.clone();
                new_status = Some(next);
                found = true;
                break;
            }
        }
        if !found {
            // If not found, still set a default toggle
            let next = desired.unwrap_or_else(|| "Open".to_string());
            g.items.push(MeshItem { name: req.name.clone(), status: next.clone() });
            new_status = Some(next);
        }
    }
    let payload = ToggleReq { name: req.name.clone(), status: new_status.clone() };
    let _ = state.app.emit("remote_toggle", payload.clone());
    broadcast_state(&state);
    Json(json!({ "ok": true, "name": req.name, "status": new_status }))
}

#[derive(Deserialize, Serialize, Clone)]
struct BatchReq { status: String }

async fn post_batch(AxumState(state): AxumState<RemoteState>, Json(req): Json<BatchReq>) -> Json<JsonValue> {
    let mut count = 0usize;
    if let Ok(mut g) = state.inner.lock() {
        for it in &mut g.items {
            it.status = req.status.clone();
            count += 1;
        }
    }
    let _ = state.app.emit("remote_batch", req.status.clone());
    broadcast_state(&state);
    Json(json!({ "ok": true, "count": count }))
}

#[derive(Deserialize, Serialize, Clone)]
struct ToggleManyReq { names: Vec<String>, status: Option<String>, apply_vis: Option<bool>, vis_var: Option<String> }

#[derive(Serialize, Clone)]
struct ToggleManyPayload { names: Vec<String>, status: String, apply_vis: bool, vis_var: Option<String> }

async fn post_toggle_many(AxumState(state): AxumState<RemoteState>, Json(req): Json<ToggleManyReq>) -> Json<JsonValue> {
    let mut desired: Option<String> = req.status.clone();
    // decide target status when not provided: if any Open -> Hide, else Open
    if desired.is_none() {
        if let Ok(g) = state.inner.lock() {
            let any_open = g.items.iter().any(|it| req.names.iter().any(|n| n == &it.name) && it.status == "Open");
            desired = Some(if any_open { "Hide".to_string() } else { "Open".to_string() });
        }
    }
    let target = desired.unwrap_or_else(|| "Open".to_string());
    let mut changed = 0usize;
    if let Ok(mut g) = state.inner.lock() {
        for it in &mut g.items {
            if req.names.iter().any(|n| n == &it.name) {
                if it.status != target { it.status = target.clone(); changed += 1; }
            }
        }
    }
    let payload = ToggleManyPayload { names: req.names.clone(), status: target.clone(), apply_vis: req.apply_vis.unwrap_or(false), vis_var: req.vis_var.clone() };
    let _ = state.app.emit("remote_toggle_many", payload);
    broadcast_state(&state);
    Json(json!({ "ok": true, "changed": changed, "status": target }))
}

#[derive(Deserialize, Serialize, Clone)]
struct PreviewModeReq { realtime: bool }

async fn post_preview_mode(AxumState(state): AxumState<RemoteState>, Json(req): Json<PreviewModeReq>) -> Json<JsonValue> {
    if let Ok(mut g) = state.inner.lock() { g.realtime = req.realtime; }
    let _ = state.app.emit("remote_preview_mode", req.realtime);
    broadcast_state(&state);
    Json(json!({ "ok": true }))
}

async fn post_preview_now(AxumState(state): AxumState<RemoteState>) -> Json<JsonValue> {
    let _ = state.app.emit("remote_preview_now", true);
    Json(json!({ "ok": true }))
}

async fn events(AxumState(state): AxumState<RemoteState>) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|msg| async move {
            match msg {
                Ok(s) => Some(Ok(Event::default().data(s))),
                Err(_e) => None,
            }
        });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"))
}

async fn index() -> ([(axum::http::header::HeaderName, String); 1], &'static str) {
    use axum::http::header::CONTENT_TYPE;
    (
        [(CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
        r#"<!doctype html>
<meta name=viewport content="width=device-width,initial-scale=1" />
<style>
  :root { color-scheme: dark; }
  body{font-family:system-ui,Segoe UI,Roboto,Arial;margin:0;padding:16px;background:#0e0f13;color:#eaeaea}
  h2{margin:0 0 12px 0}
  .row{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin:10px 0}
  .pill{padding:4px 10px;border:1px solid #3a3f4b;border-radius:999px;background:#151823}
  .btn{padding:8px 12px;border-radius:8px;border:1px solid #3a3f4b;background:#1f2430;color:#fff;cursor:pointer}
  .btn.success{border-color:#2e7d32;background:#1b5e20}
  .btn.danger{border-color:#b71c1c;background:#7f0000}
  .btn.warn{border-color:#ef6c00;background:#e65100}
  .btn.toggle[data-on="1"]{background:#245a24}
  .btn:disabled{opacity:.6;cursor:default}
  /* Segmented control for Preview Mode */
  .pm-label{font-weight:600;color:#c9d1d9;margin-right:8px}
  .seg{display:inline-flex;gap:2px;border:1px solid #3a3f4b;border-radius:999px;padding:2px;background:#0f1320}
  .seg-btn{padding:6px 12px;border:none;border-radius:999px;background:transparent;color:#c9d1d9;cursor:pointer}
  .seg-btn.active{background:#5b7cff;color:#fff}
  /* Big Preview button */
  .btn.preview-big{border-color:#ef6c00;background:transparent;color:#efb566;padding:10px 18px;border-width:2px;border-radius:999px}
  .btn.preview-big:hover{background:rgba(239,108,0,.15)}
  /* Warning note */
  .warn-note{color:#ff8a80;font-weight:600}
  .table{width:100%;border-collapse:collapse;margin-top:8px}
  .table th,.table td{border-bottom:1px solid #2a2f3b;padding:8px 10px;text-align:left}
  .table th{color:#b0b8c0;font-weight:600}
  .status-pill{padding:2px 8px;border-radius:999px;border:1px solid #3a3f4b;background:#151823}
  .spacer{flex:1}
  input[type="text"].mono{font-family:ui-monospace,Consolas,Menlo,monospace;background:#0e0f13;color:#eaeaea;border:1px solid #3a3f4b;border-radius:6px;padding:6px 8px;min-width:260px}
  /* Category header */
  .section{background:#0f1320;color:#b9c1c9; cursor:pointer}
  .section td{padding-top:14px;font-weight:600}
  .sec-hdr{display:flex;align-items:center;gap:8px}
  .twisty{display:inline-flex;align-items:center;justify-content:center;width:18px;height:18px;border:1px solid #3a3f4b;border-radius:3px;background:#1a1f2b;font-size:12px;color:#b9c1c9}
  .twisty:hover{filter:brightness(1.1)}
  /* Section row coloring */
  tr.section[data-state="Open"] td{background:rgba(46,125,50,.22); border-left:3px solid #2e7d32}
  tr.section[data-state="Hide"] td{background:rgba(183,28,28,.18); border-left:3px solid #b71c1c}
  tr.section[data-state="Mixed"] td{background:rgba(239,108,0,.18); border-left:3px solid #ef6c00}
  /* Row state coloring and clickable rows */
  tbody tr.item{cursor:pointer; transition: background .12s ease}
  tbody tr.item[data-state="Open"]{background:rgba(46,125,50,.18); border-left:3px solid #2e7d32}
  tbody tr.item[data-state="Hide"]{background:rgba(183,28,28,.12); border-left:3px solid #b71c1c}
  tbody tr.item:hover{filter:brightness(1.05)}
  tbody tr.item.disabled{cursor:not-allowed; opacity:.85}
</style>
<h2>OwlTools Remote</h2>
<div class=row>
  <span class=pill id=path></span>
  <span class=spacer></span>
  <button class="btn" id=refreshBtn>Refresh</button>
  <button class="btn success" id=showAllBtn>Show All</button>
  <button class="btn danger" id=hideAllBtn>Hide All</button>
</div>
<div class=row>
  <span class="pm-label">Preview Mode:</span>
  <div class="seg">
    <button class="seg-btn" id=pmReal>Realtime</button>
    <button class="seg-btn" id=pmUnreal>Un-realtime</button>
  </div>
  <span class=spacer></span>
</div>
<div class=row>
  <span class="warn-note">โปรดกด Show All ก่อนส่งตรวจงาน</span>
  <span class=spacer></span>
  <button class="btn preview-big" id=previewBtn style="display:none">Preview</button>
</div>

<table class=table>
  <thead>
    <tr><th>Name</th><th>Status</th><th>Action</th></tr>
  </thead>
  <tbody id=tbody></tbody>
  </table>

<script>
let state = { items: [], file_path: '', realtime: true, tunnel_url: '' };
let es;
const expandedSections = {};
// grouping helpers
const isVisNode = (name)=> /FDST_VIS/i.test(name) || /_VIS\-/i.test(name);
const detectZone = (name)=>{ const m = /(?:^|[._\-\s])ID[\-_]?([A-Z])/i.exec(name); return m? m[1].toUpperCase(): null; };
const isDecalName = (lname)=> /(\b|[^a-z0-9])decals?(\b|[^a-z0-9])/i.test(lname) || /[\-_]decals?[\-_]/i.test(lname) || /(^|[^a-z0-9])(base|[a-z])1([^a-z0-9]|$)/i.test(lname);

function groupItems(items){
  const vis=[], bases=[], bricks=[], decals=[], singles=[]; const zones={};
  for (const it of items){
    const lname = it.name.toLowerCase();
    const z = detectZone(it.name);
    if (lname.includes('brick')) { (bricks.push(it)); continue; }
    if (isVisNode(it.name)) { (vis.push(it)); if (z) ((zones[z] ||= []).push(it)); continue; }
    if (isDecalName(lname)) { (decals.push(it)); if (z) ((zones[z] ||= []).push(it)); continue; }
    if (lname.includes('base')) { (bases.push(it)); continue; }
    if (z) { (zones[z] ||= []).push(it); continue; }
    singles.push(it);
  }
  const sections = [];
  for (const z of Object.keys(zones).sort()) sections.push({ title: `Zone${z}`, items: zones[z] });
  if (vis.length) sections.push({ title:'VIS', items: vis });
  if (bases.length) sections.push({ title:'Base', items: bases });
  if (bricks.length) sections.push({ title:'Brick', items: bricks });
  if (decals.length) sections.push({ title:'Decals', items: decals });
  if (singles.length) sections.push({ title:'Singles', items: singles });
  return sections;
}

function render(){
  document.getElementById('path').textContent = state.file_path || '';
  const tb = document.getElementById('tbody'); tb.innerHTML = '';
  const sections = groupItems(state.items||[]);
  for (const sec of sections){
    const sh = document.createElement('tr'); sh.className='section';
    const td = document.createElement('td'); td.colSpan=3; sh.appendChild(td); tb.appendChild(sh);
    const hdr = document.createElement('div'); hdr.className='sec-hdr'; td.appendChild(hdr);
    const tw = document.createElement('span'); tw.className='twisty'; hdr.appendChild(tw);
    const title = document.createElement('span'); title.textContent = sec.title; hdr.appendChild(title);
    // compute section aggregate state for coloring
    let openCnt = 0, hideCnt = 0;
    for (const it of sec.items){ if (it.status === 'Open') openCnt++; else hideCnt++; }
    const gstate = openCnt && hideCnt ? 'Mixed' : (openCnt ? 'Open' : 'Hide');
    sh.dataset.state = gstate;
    const expanded = (sec.title in expandedSections) ? !!expandedSections[sec.title] : false;
    tw.textContent = expanded ? '▼' : '▶';
    tw.onclick = (ev)=>{ ev.stopPropagation(); expandedSections[sec.title] = !expanded; render(); };
    let names = sec.items.filter(it=>!isVisNode(it.name)).map(it=>it.name);
    const apply_vis = (sec.title === 'Base') || (sec.title === 'Brick') || /^Zone[A-Z]$/.test(sec.title);
    let vis_var = null;
    if (sec.title === 'Base') vis_var = 'Base';
    else {
      const mz = /^Zone([A-Z])$/.exec(sec.title); if (mz) vis_var = mz[1];
    }
    if (sec.title === 'Base') {
      // include base-related decals anywhere
      const extras = state.items.filter(it=>!isVisNode(it.name) && /base/i.test(it.name)).map(it=>it.name);
      names = Array.from(new Set([...names, ...extras]));
    }
    sh.onclick = async()=>{
      await fetch('/api/toggle_many',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({names, apply_vis, vis_var})});
    };
    if (!expanded) continue;
    for (const it of sec.items){
      const tr = document.createElement('tr'); tr.className='item'; tr.dataset.state = it.status;
      const isVis = isVisNode(it.name);
      if (isVis) tr.classList.add('disabled');
      const tdName = document.createElement('td'); tdName.textContent = it.name; tr.appendChild(tdName);
      const tdStatus = document.createElement('td'); const sp = document.createElement('span'); sp.className='status-pill'; sp.textContent = it.status; tdStatus.appendChild(sp); tr.appendChild(tdStatus);
      const tdAct = document.createElement('td'); const b = document.createElement('button'); b.className='btn';
      const next = it.status==='Open'?'Hide':'Open'; b.textContent = next;
      b.disabled = isVis;
      b.onclick = async(ev)=>{
        ev.stopPropagation();
        if (isVis) return;
        const want = it.status==='Open'?'Hide':'Open';
        await fetch('/api/toggle',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:it.name,status:want})});
      };
      tdAct.appendChild(b); tr.appendChild(tdAct);
      tr.onclick = async()=>{
        if (isVis) return;
        const want = it.status==='Open'?'Hide':'Open';
        await fetch('/api/toggle',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:it.name,status:want})});
      };
      tb.appendChild(tr);
    }
  }
  // preview controls
  const rbtn = document.getElementById('pmReal');
  const ubtn = document.getElementById('pmUnreal');
  if (rbtn && ubtn){
    if (state.realtime){ rbtn.classList.add('active'); ubtn.classList.remove('active'); }
    else { rbtn.classList.remove('active'); ubtn.classList.add('active'); }
  }
  const pv = document.getElementById('previewBtn');
  pv.style.display = state.realtime? 'none':'inline-block';
}

async function refresh(){
  const r = await fetch('/api/mesh');
  const j = await r.json();
  state.items = j.items||[]; state.file_path = j.file_path||''; state.realtime = !!j.realtime; state.tunnel_url = j.tunnel_url||'';
  render();
}

function connectSSE(){
  try { if (es) es.close(); } catch{}
  es = new EventSource('/api/events');
  es.onmessage = (ev)=>{
    try {
      const j = JSON.parse(ev.data);
      if (j && typeof j === 'object'){
        state.items = j.items||[]; state.file_path = j.file_path||''; state.realtime = !!j.realtime; state.tunnel_url = j.tunnel_url||'';
        render();
      }
    } catch{}
  };
  es.onerror = ()=>{ try{ es.close(); }catch{}; setTimeout(connectSSE, 2000); };
}

async function batch(status){
  await fetch('/api/batch',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({status})});
}
async function setRealtime(v){
  try{
    await fetch('/api/preview_mode',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({realtime:v})});
  } finally {
    // Optimistic UI update; SSE will reconcile if needed
    state.realtime = !!v; render();
  }
}
async function previewNow(){ await fetch('/api/preview_now',{method:'POST'}); }

document.getElementById('refreshBtn').onclick = refresh;
document.getElementById('showAllBtn').onclick = ()=>batch('Open');
document.getElementById('hideAllBtn').onclick = ()=>batch('Hide');
document.getElementById('pmReal').onclick = ()=> setRealtime(true);
document.getElementById('pmUnreal').onclick = ()=> setRealtime(false);
document.getElementById('previewBtn').onclick = ()=> previewNow();

// initial
refresh(); connectSSE();
</script>
"#,
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let version = app.package_info().version.to_string();
                let display_version = option_env!("OWLTOOLS_BUILD_TAG").unwrap_or(version.as_str());
                let _ = window.set_title(&format!("OwlTools for Enfusion Engine - {}", display_version));
            }
            // start remote control server
            let app_handle = app.handle();
            let (tx, _rx) = broadcast::channel(32);
            let remote = RemoteState { app: app_handle.clone(), inner: Arc::new(Mutex::new(RemoteData::default())), tx };
            app.manage(remote.clone());

            tauri::async_runtime::spawn(async move {
                let router = Router::new()
                    .route("/", get(index))
                    .route("/api/mesh", get(get_mesh))
                    .route("/api/toggle", post(post_toggle))
                    .route("/api/toggle_many", post(post_toggle_many))
                    .route("/api/batch", post(post_batch))
                    .route("/api/preview_mode", post(post_preview_mode))
                    .route("/api/preview_now", post(post_preview_now))
                    .route("/api/events", get(events))
                    .with_state(remote)
                    .layer(CorsLayer::permissive());
                let addr = "0.0.0.0:8787";
                let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
                let _ = axum::serve(listener, router).await;
            });
            // Window menu
            let menu = tauri::menu::MenuBuilder::new(app)
                .text("about", "About OwlTools")
                .separator()
                .text("quit", "Quit")
                .build()?;
            app.set_menu(menu)?;

            // System tray with menu
            let show_i = tauri::menu::MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = tauri::menu::MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = tauri::menu::Menu::with_items(app, &[&show_i, &quit_i])?;

            tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| match event {
                    tauri::tray::TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            // Handle window menu events
            app.on_menu_event(|app_handle, event| match event.id().as_ref() {
                "quit" => app_handle.exit(0),
                "about" => {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_display_version,
            greet,
            read_text_file,
            write_text_file,
            wb_call,
            updater_download_msi,
            updater_install_msi,
            set_mesh_state,
            set_realtime_mode,
            start_quick_tunnel,
            start_quick_tunnel_unique,
            stop_quick_tunnel,
            get_prefab_cache_status,
            auto_detect_svn_root,
            scan_prefab_index,
            remember_svn_root,
            get_autosocket_settings,
            get_autosocket_presets,
            save_autosocket_presets,
            remember_save_dir,
            remember_extra_dirs,
            remember_blender_path,
            remember_ebt_addons_dir,
            open_fbx_in_blender,
            mqa_report_from_xob,
            create_new_et_from_xob,
            suggest_prefab_folders_from_xob,
            create_new_et_with_meta_from_xob,
            prefabdst_build,
            prefabdst_scan_dst,
            prefabdst_scan_full_dst,
            prefabdst_read_meta,
            
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
