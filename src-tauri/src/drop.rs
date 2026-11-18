//! Drop: shares dropped files over the local network (and receives uploads) behind a random URL token.

use crate::AppState;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    net::{IpAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tiny_http::{Header, Method, Request, Response, Server};

const PORT: u16 = 27824;
const LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The open Drop session. The server itself never stops (see `ensure_server`); closing Drop drops this,
/// which turns every request into a 404.
pub struct Active {
    token: String,
    files: Arc<Vec<PathBuf>>,
    inbox: PathBuf,
}

pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
        IpAddr::V6(v) => {
            let s = v.segments()[0];
            v.is_loopback()
                || s & 0xfe00 == 0xfc00
                || s & 0xffc0 == 0xfe80
                || v.to_ipv4_mapped().is_some_and(|m| is_local(IpAddr::V4(m)))
        }
    }
}

/// The address of the interface that holds the default route. UDP connect sends no packet.
fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    // Link-local (169.254.x) is what a disconnected adapter holds: a phone can't reach it.
    Some(s.local_addr().ok()?.ip()).filter(|ip| is_local(*ip) && !ip.is_loopback() && !matches!(ip, IpAddr::V4(v) if v.is_link_local()))
}

/// A safe Windows file name from whatever the client sent, or None.
pub fn sanitize(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let s: String = base.chars().filter(|c| !c.is_control() && !r#"<>:"|?*"#.contains(*c)).take(200).collect();
    let s = s.trim().trim_end_matches(['.', ' ']);
    if s.is_empty() || s.chars().all(|c| c == '.') {
        return None;
    }
    let stem = s.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    Some(if reserved { format!("_{s}") } else { s.to_string() })
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |j: usize| b.get(j).and_then(|c| (*c as char).to_digit(16));
        match (b[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// First free `dir/name`, `dir/name (1).ext`, … created atomically so concurrent uploads never clobber.
fn create_unique(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 0..1000 {
        let p = dir.join(if n == 0 { name.to_string() } else { format!("{stem} ({n}){ext}") });
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("demasiados archivos con ese nombre"))
}

/// A download body that reports `drop-sending` {name, sent, total} on each whole percent and
/// `drop-sent` once the last byte has been read.
struct Progress<'a> {
    file: File,
    name: String,
    sent: u64,
    total: u64,
    pct: u64,
    on_event: &'a dyn Fn(&str, Value),
}

impl Read for Progress<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.sent += n as u64;
        let pct = if self.total == 0 { 100 } else { self.sent * 100 / self.total };
        if n > 0 && pct != self.pct {
            self.pct = pct;
            (self.on_event)("drop-sending", json!({ "name": self.name, "sent": self.sent, "total": self.total }));
        }
        // shortcut: "read to the end" means handed to the socket, not confirmed by the phone; fine on a LAN.
        if n == 0 && self.sent == self.total && self.pct != u64::MAX {
            self.pct = u64::MAX;
            (self.on_event)("drop-sent", json!(self.name));
        }
        Ok(n)
    }
}

pub fn start(app: &AppHandle, paths: Vec<PathBuf>) -> Result<(), String> {
    if !app.state::<AppState>().cfg.lock().unwrap().drop {
        return Err("Compartir archivos está desactivado en Ajustes".into());
    }
    stop(app);
    let files: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
    let ip = lan_ip().ok_or("No hay conexión a una red local")?;
    ensure_server(app)?;
    let token = crate::random_hex(16);
    let url = format!("http://{ip}:{PORT}/{token}/");
    let qr = qrcode::QrCode::new(url.as_bytes())
        .map_err(|e| e.to_string())?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(148, 148)
        .quiet_zone(true)
        .build();
    let inbox = app.path().download_dir().map_err(|e| e.to_string())?.join("Drop");
    let names: Vec<String> = files.iter().map(|p| file_name(p)).collect();
    *app.state::<AppState>().drop.lock().unwrap() =
        Some(Active { token: token.clone(), files: Arc::new(files), inbox: inbox.clone() });
    let _ = app.emit_to("island", "drop", json!({
        "url": url,
        "qr": qr,
        "files": names,
        "inbox": inbox.display().to_string(),
    }));

    let a = app.clone();
    thread::spawn(move || {
        thread::sleep(LIFETIME);
        let current = a.state::<AppState>().drop.lock().unwrap().as_ref().map(|d| d.token.clone());
        if current.as_deref() == Some(token.as_str()) {
            stop(&a);
        }
    });
    Ok(())
}

/// Starts Drop with the files the island page posts on a drop (ui/main.js). WebView2 handles the
/// drag itself: wry's OLE drop target (dragDropEnabled) never received drags from Explorer here.
pub fn listen_native_drops(app: &AppHandle) -> tauri::Result<()> {
    use webview2_com::{
        take_pwstr,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2},
        WebMessageReceivedEventHandler,
    };
    use windows_core::{Interface, PWSTR};
    let Some(w) = app.get_webview_window("island") else { return Ok(()) };
    let app = app.clone();
    w.with_webview(move |pw| unsafe {
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut msg = PWSTR::null();
            if args.TryGetWebMessageAsString(&mut msg).is_err() || take_pwstr(msg) != "island-drop" {
                return Ok(());
            }
            let objs = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>()?.AdditionalObjects()?;
            let mut n = 0;
            objs.Count(&mut n)?;
            let mut paths = Vec::new();
            for i in 0..n {
                if let Ok(f) = objs.GetValueAtIndex(i)?.cast::<ICoreWebView2File>() {
                    let mut p = PWSTR::null();
                    f.Path(&mut p)?;
                    paths.push(PathBuf::from(take_pwstr(p)));
                }
            }
            if let Err(e) = start(&app, paths) {
                let _ = app.emit_to("island", "drop-error", e);
            }
            Ok(())
        }));
        let mut token = 0i64;
        let added = pw.controller().CoreWebView2().and_then(|core| core.add_WebMessageReceived(&handler, &mut token));
        if let Err(e) = added {
            eprintln!("drop listener: {e}");
        }
    })
}

/// Binds the Drop port once and keeps it for the app's lifetime. tiny_http can't be shut down cleanly
/// on Windows while a client is still connected (a phone's half-open attempt was enough): the port
/// stayed taken and every later Drop failed with error 10048.
fn ensure_server(app: &AppHandle) -> Result<(), String> {
    static STARTED: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut started = STARTED.lock().unwrap();
    if *started {
        return Ok(());
    }
    let server = Server::http(("0.0.0.0", PORT)).map_err(|e| format!("No se pudo abrir el puerto {PORT}: {e}"))?;
    let a = app.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let a = a.clone();
            // One thread per request so a long download doesn't block the page.
            thread::spawn(move || {
                let session = a.state::<AppState>().drop.lock().unwrap().as_ref().map(|d| (d.token.clone(), d.files.clone(), d.inbox.clone()));
                match session {
                    Some((t, files, inbox)) => {
                        handle(req, &t, &files, &inbox, &|event, payload| drop(a.emit_to("island", event, payload)))
                    }
                    None => drop(req.respond(Response::empty(404))),
                }
            });
        }
    });
    *started = true;
    Ok(())
}

pub fn stop(app: &AppHandle) {
    if app.state::<AppState>().drop.lock().unwrap().take().is_some() {
        let _ = app.emit_to("island", "drop", ());
    }
}

#[tauri::command]
pub fn drop_stop(app: AppHandle) {
    stop(&app);
}

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header")
}

/// `on_event(name, payload)`: `drop-received` (file name), `drop-sending`, `drop-sent` (see `Progress`).
fn handle(mut req: Request, token: &str, files: &[PathBuf], inbox: &Path, on_event: &dyn Fn(&str, Value)) {
    let empty = |code| Response::empty(code).boxed();
    if !req.remote_addr().is_some_and(|a| is_local(a.ip())) {
        let _ = req.respond(empty(403));
        return;
    }
    let url = req.url().to_string();
    let Some(rest) = url.strip_prefix(&format!("/{token}/")) else {
        let _ = req.respond(empty(404));
        return;
    };
    let resp = match (req.method(), rest) {
        (Method::Get, "") => Response::from_string(page(files))
            .with_header(header("Content-Type", "text/html; charset=utf-8"))
            .boxed(),
        (Method::Get, r) if r.starts_with("f/") => {
            let file = r[2..].parse::<usize>().ok().and_then(|i| files.get(i)).and_then(|p| {
                let f = File::open(p).ok()?;
                Some((p, f.metadata().ok()?.len(), f))
            });
            match file {
                Some((p, total, file)) => {
                    let name = file_name(p);
                    let headers = vec![
                        header("Content-Type", "application/octet-stream"),
                        header("Content-Disposition", &format!("attachment; filename*=UTF-8''{}", percent_encode(&name))),
                    ];
                    // Not boxed: the body borrows on_event, and respond() streams it right here.
                    let body = Progress { file, name, sent: 0, total, pct: 0, on_event };
                    let _ = req.respond(Response::new(200.into(), headers, body, Some(total as usize), None));
                    return;
                }
                None => empty(404),
            }
        }
        (Method::Put, r) if r.starts_with("u/") => match sanitize(&percent_decode(&r[2..])) {
            None => empty(400),
            Some(name) => {
                let saved = fs::create_dir_all(inbox).and_then(|_| create_unique(inbox, &name)).and_then(|(p, mut f)| {
                    // Don't leave a truncated file behind if the upload breaks off.
                    io::copy(req.as_reader(), &mut f).map(|_| p.clone()).inspect_err(|_| {
                        drop(fs::remove_file(&p));
                    })
                });
                match saved {
                    Ok(p) => {
                        on_event("drop-received", json!(file_name(&p)));
                        empty(201)
                    }
                    Err(_) => empty(500),
                }
            }
        },
        _ => empty(404),
    };
    let _ = req.respond(resp);
}

fn page(files: &[PathBuf]) -> String {
    let list: String = files
        .iter()
        .enumerate()
        .map(|(i, p)| format!(r#"<li><a href="f/{i}" download>{}</a></li>"#, html_escape(&file_name(p))))
        .collect();
    let list = if list.is_empty() { "<li class=muted>No hay archivos para descargar.</li>".into() } else { list };
    format!(
        r#"<!doctype html><html lang="es"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Drop</title>
<style>
:root{{color-scheme:light dark;--bg:#fff;--fg:#111;--dim:#666;--line:#ddd;--accent:#0a66ff}}
@media (prefers-color-scheme:dark){{:root{{--bg:#000;--fg:#f5f5f7;--dim:#8e8e93;--line:#2c2c2e;--accent:#4c8dff}}}}
body{{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:16px/1.4 system-ui,sans-serif;max-width:560px;margin-inline:auto}}
h1{{font-size:22px;margin:0 0 20px}}h2{{font-size:15px;color:var(--dim);font-weight:600;margin:28px 0 8px}}
ul{{list-style:none;padding:0;margin:0}}li{{border-bottom:1px solid var(--line)}}
li a{{display:block;padding:14px 0;color:var(--accent);text-decoration:none;word-break:break-all}}.muted{{color:var(--dim);padding:14px 0}}
label{{display:block;padding:16px;border:1px dashed var(--line);border-radius:12px;text-align:center;color:var(--accent)}}
input{{display:none}}#status{{color:var(--dim);margin-top:12px;white-space:pre-line}}
</style></head><body>
<h1>Drop</h1>
<h2>Descargar</h2><ul>{list}</ul>
<h2>Enviar al PC</h2>
<label>Elegir archivos<input type="file" id="pick" multiple></label>
<div id="status" role="status"></div>
<script>
const status=document.getElementById('status');
document.getElementById('pick').addEventListener('change',async e=>{{
  for(const f of e.target.files){{
    status.textContent+='Enviando '+f.name+'…\n';
    try{{const r=await fetch('u/'+encodeURIComponent(f.name),{{method:'PUT',body:f}});
      status.textContent+=(r.ok?'✓ ':'✗ ')+f.name+'\n';}}
    catch{{status.textContent+='✗ '+f.name+' (sin conexión)\n';}}
  }}
  e.target.value='';
}});
</script></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_traversal_and_reserved_names() {
        assert_eq!(sanitize("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(sanitize(r"..\..\Windows\win.ini").as_deref(), Some("win.ini"));
        assert_eq!(sanitize("foto: 1?.jpg").as_deref(), Some("foto 1.jpg"));
        assert_eq!(sanitize("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(sanitize("COM1").as_deref(), Some("_COM1"));
        assert_eq!(sanitize("..").as_deref(), None);
        assert_eq!(sanitize("a/").as_deref(), None);
        assert_eq!(sanitize("nota. ").as_deref(), Some("nota"));
        assert_eq!(sanitize("canción ñ.mp3").as_deref(), Some("canción ñ.mp3"));
    }

    #[test]
    fn percent_round_trip() {
        assert_eq!(percent_decode("canci%C3%B3n%20%281%29.mp3"), "canción (1).mp3");
        assert_eq!(percent_decode("bad%zz%"), "bad%zz%");
        assert_eq!(percent_decode(&percent_encode("a b/ñ.txt")), "a b/ñ.txt");
    }

    #[test]
    fn serves_page_files_and_uploads_over_http() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("di-drop-test-{}", crate::random_hex(4)));
        fs::create_dir_all(&dir).unwrap();
        let shared = dir.join("a b.txt");
        fs::write(&shared, "hola").unwrap();
        let inbox = dir.join("inbox");
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let client = thread::spawn(move || {
            let req = |line: &str, extra: &str, body: &[u8]| {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "{line} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra}\r\n").unwrap();
                s.write_all(body).unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).unwrap();
                out
            };
            [
                req("GET /tok/", "", b""),
                req("GET /tok/f/0", "", b""),
                req("PUT /tok/u/..%2Fcanci%C3%B3n.txt", "Content-Length: 5\r\n", b"datos"),
                req("GET /otro/", "", b""),
                req("GET /tok/f/9", "", b""),
            ]
        });
        let got = std::sync::Mutex::new(vec![]);
        for _ in 0..5 {
            handle(server.recv().unwrap(), "tok", &[shared.clone()], &inbox, &|e, v| got.lock().unwrap().push(format!("{e} {v}")));
        }
        let [page, file, up, bad_token, bad_index] = client.join().unwrap();
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("a b.txt"), "{page}");
        assert!(file.contains("filename*=UTF-8''a%20b.txt") && file.ends_with("hola"), "{file}");
        assert!(up.starts_with("HTTP/1.1 201"), "{up}");
        assert_eq!(fs::read_to_string(inbox.join("canción.txt")).unwrap(), "datos");
        assert_eq!(
            *got.lock().unwrap(),
            [r#"drop-sending {"name":"a b.txt","sent":4,"total":4}"#, r#"drop-sent "a b.txt""#, r#"drop-received "canción.txt""#]
        );
        assert!(bad_token.starts_with("HTTP/1.1 404"), "{bad_token}");
        assert!(bad_index.starts_with("HTTP/1.1 404"), "{bad_index}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_local_addresses() {
        for ok in ["192.168.1.5", "10.0.0.2", "172.16.4.4", "127.0.0.1", "169.254.1.1", "fe80::1", "fd00::1"] {
            assert!(is_local(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "100.64.0.1"] {
            assert!(!is_local(bad.parse().unwrap()), "{bad}");
        }
    }
}
