#![windows_subsystem = "windows"]
use eframe::egui::{self, Color32, RichText};
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, error::Error, fs, path::{Path, PathBuf}, process::Command, sync::{Arc, Mutex, OnceLock}};

type R<T> = Result<T, Box<dyn Error + Send + Sync>>;
const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JAVA_ALL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const FIRST: &str = "1.8";
const LAST: &str = "26.3";
const ACCENT: Color32 = Color32::from_rgb(59, 130, 246);
const CARD: Color32 = Color32::from_rgb(31, 34, 40);
const PACK: &[&str] = &["sodium", "lithium", "iris", "modmenu", "zoomify", "ferrite-core", "entityculling",
    "immediatelyfast", "sodium-extra", "appleskin", "xaeros-minimap", "betterf3", "dynamic-fps", "mouse-tweaks"];

#[derive(Clone)]
struct Hit { slug: String, title: String, author: String, desc: String, dl: u64, icon: Option<egui::ColorImage> }
#[derive(Default)]
struct Shared { versions: Vec<(String, String)>, status: String, busy: bool, progress: f32,
    results: Vec<Hit>, installed: HashSet<String>, installing: usize, total: u64,
    acct: Option<Acct>, login: String, head: Option<egui::ColorImage>, guest: Option<egui::ColorImage>,
    guest_full: Option<(image::RgbaImage, bool)>, acct_skin: Option<(image::RgbaImage, bool)>,
    defaults: Vec<(String, image::RgbaImage, bool)>, picked: Vec<(u8, PathBuf)>, ended: Vec<(String, u64)>,
    launched: bool, meta: Option<(String, Vec<String>, Vec<String>)>, cloud_in: Option<Value> }

fn root() -> PathBuf {
    let b = std::env::var("APPDATA").or_else(|_| std::env::var("HOME")).unwrap_or(".".into());
    PathBuf::from(b).join(".projectwarden")
}
fn client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::blocking::Client::builder().user_agent("ProjectWarden/0.3 (launcher)").connect_timeout(std::time::Duration::from_secs(10)).build().unwrap())
}
fn get(url: &str) -> R<Vec<u8>> {
    let mut last: R<Vec<u8>> = Err("hiba".into());
    for _ in 0..3 {
        last = (|| -> R<Vec<u8>> { Ok(client().get(url).send()?.error_for_status()?.bytes()?.to_vec()) })();
        if last.is_ok() { break; }
    }
    last
}
fn json(url: &str) -> R<Value> { Ok(serde_json::from_slice(&get(url)?)?) }
fn dl(url: &str, p: &Path) -> R<()> {
    if p.exists() { return Ok(()); }
    fs::create_dir_all(p.parent().unwrap())?;
    let mut t = p.as_os_str().to_owned(); t.push(".part");
    let t = PathBuf::from(t);
    fs::write(&t, get(url)?)?;
    fs::rename(&t, p)?;
    Ok(())
}
fn maven(n: &str) -> String {
    let p: Vec<&str> = n.split(':').collect();
    format!("{}/{}/{}/{}-{}.jar", p[0].replace('.', "/"), p[1], p[2], p[1], p[2])
}
fn set(sh: &Arc<Mutex<Shared>>, s: &str) { sh.lock().unwrap().status = s.into(); }
fn prog(sh: &Arc<Mutex<Shared>>, p: f32) { sh.lock().unwrap().progress = p; }
fn inst_dir(id: &str, fabric: bool) -> PathBuf {
    root().join("instances").join(format!("{id}-{}", if fabric { "fabric" } else { "vanilla" }))
}
fn allowed(rules: Option<&Value>) -> bool {
    let Some(rs) = rules.and_then(|r| r.as_array()) else { return true };
    let mut ok = false;
    for r in rs {
        if r.get("features").is_some() { continue; }
        let m = match r.get("os") {
            None => true,
            Some(o) => o.get("name").map_or(true, |n| n == "windows") && o.get("arch").is_none(),
        };
        if m { ok = r["action"] == "allow"; }
    }
    ok
}
fn load_versions() -> R<Vec<(String, String)>> {
    let m = json(MANIFEST)?;
    let rel: Vec<(String, String)> = m["versions"].as_array().unwrap().iter()
        .filter(|v| v["type"] == "release")
        .map(|v| (v["id"].as_str().unwrap().into(), v["url"].as_str().unwrap().into())).collect();
    let a = rel.iter().position(|v| v.0 == LAST).unwrap_or(0);
    let b = rel.iter().position(|v| v.0 == FIRST).unwrap_or(rel.len() - 1);
    Ok(rel[a..=b].to_vec())
}
fn load_cfg() -> (String, u32, String, bool) {
    let t = fs::read_to_string(root().join("settings.txt")).unwrap_or_default();
    let l: Vec<&str> = t.lines().collect();
    let name = l.get(0).map(|s| s.to_string()).filter(|s| !s.is_empty()).unwrap_or("Player".into());
    (name, l.get(1).and_then(|x| x.parse().ok()).unwrap_or(4096), l.get(2).map(|s| s.to_string()).unwrap_or_default(),
     l.get(3).map_or(true, |x| *x == "true"))
}
fn save_cfg(name: &str, ram: u32, java: &str, fabric: bool) {
    let _ = fs::create_dir_all(root());
    let _ = fs::write(root().join("settings.txt"), format!("{name}\n{ram}\n{java}\n{fabric}"));
}

fn install_mod(slug: &str, mc: &str, dir: &Path, seen: &mut HashSet<String>) -> R<()> {
    if !seen.insert(slug.to_string()) { return Ok(()); }
    let gv = format!("[\"{mc}\"]");
    let vs: Value = client().get(format!("https://api.modrinth.com/v2/project/{slug}/version"))
        .query(&[("loaders", r#"["fabric"]"#), ("game_versions", gv.as_str())]).send()?.error_for_status()?.json()?;
    let v = vs.get(0).ok_or("no compatible version")?;
    for d in v["dependencies"].as_array().into_iter().flatten() {
        if d["dependency_type"] == "required" {
            if let Some(p) = d["project_id"].as_str() { let _ = install_mod(p, mc, dir, seen); }
        }
    }
    let f = v["files"].as_array().and_then(|a| a.iter().find(|f| f["primary"] == true).or(a.first())).ok_or("no file")?;
    dl(f["url"].as_str().unwrap(), &dir.join(f["filename"].as_str().unwrap()))
}
fn spawn_install(sh: &Arc<Mutex<Shared>>, slugs: Vec<String>, mc: String, dir: PathBuf) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        let _ = fs::create_dir_all(&dir);
        let mut seen = HashSet::new();
        let mut bad = 0;
        sh.lock().unwrap().installing += 1;
        for s in &slugs {
            set(&sh, &format!("Installing: {s}..."));
            if install_mod(s, &mc, &dir, &mut seen).is_ok() { sh.lock().unwrap().installed.insert(format!("{mc}|{s}")); } else { bad += 1; }
        }
        sh.lock().unwrap().installing -= 1;
        set(&sh, &if bad == 0 { "Done, installed.".to_string() } else { format!("{bad} item(s) not available for this version.") });
    });
}
fn fetch_icon(url: &str) -> R<egui::ColorImage> {
    let img = image::load_from_memory(&get(url)?)?.thumbnail(64, 64).to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([img.width() as _, img.height() as _], &img))
}
fn spawn_search(sh: &Arc<Mutex<Shared>>, q: String, mc: String, page: usize) {
    let sh = sh.clone();
    let off = (page * 20).to_string();
    std::thread::spawn(move || {
        let facets = format!(r#"[["project_type:mod"],["categories:fabric"],["versions:{mc}"]]"#);
        let res = (|| -> R<Value> {
            Ok(client().get("https://api.modrinth.com/v2/search")
                .query(&[("query", q.as_str()), ("facets", facets.as_str()), ("limit", "20"), ("offset", off.as_str())])
                .send()?.error_for_status()?.json()?)
        })();
        match res {
            Ok(r) => {
                sh.lock().unwrap().total = r["total_hits"].as_u64().unwrap_or(0);
                let arr = r["hits"].as_array().cloned().unwrap_or_default();
                let s = |h: &Value, k: &str| h[k].as_str().unwrap_or("").to_string();
                let hits: Vec<Hit> = arr.iter().map(|h| Hit { slug: s(h, "slug"), title: s(h, "title"), author: s(h, "author"),
                    desc: s(h, "description"), dl: h["downloads"].as_u64().unwrap_or(0), icon: None }).collect();
                sh.lock().unwrap().results = hits.clone();
                for (i, h) in arr.iter().enumerate() {
                    let u = s(h, "icon_url");
                    if u.is_empty() { continue; }
                    if let Ok(ci) = fetch_icon(&u) {
                        let mut g = sh.lock().unwrap();
                        if let Some(x) = g.results.get_mut(i) { if x.slug == hits[i].slug { x.icon = Some(ci); } }
                    }
                }
            }
            Err(e) => set(&sh, &format!("Search error: {e}")),
        }
    });
}

fn ensure_java(comp: &str, sh: &Arc<Mutex<Shared>>) -> R<PathBuf> {
    let dir = root().join("runtime").join(comp);
    let exe = dir.join("bin").join("java.exe");
    let ok = dir.join(".ok");
    if ok.exists() && exe.exists() { return Ok(exe); }
    set(sh, "Downloading Java (one time only)...");
    let all = json(JAVA_ALL)?;
    let url = all["windows-x64"][comp][0]["manifest"]["url"].as_str().ok_or("Java not found")?;
    let man = json(url)?;
    let files: Vec<(String, String)> = man["files"].as_object().unwrap().iter().filter(|(_, v)| v["type"] == "file")
        .map(|(k, v)| (k.clone(), v["downloads"]["raw"]["url"].as_str().unwrap_or("").to_string())).collect();
    std::thread::scope(|s| {
        for ch in files.chunks(files.len() / 8 + 1) {
            let dir = &dir;
            s.spawn(move || for (p, u) in ch { if !u.is_empty() { let _ = dl(u, &dir.join(p)); } });
        }
    });
    if !exe.exists() { return Err("Java download failed".into()); }
    fs::write(ok, "ok")?;
    Ok(exe)
}

fn launch(id: &str, url: &str, inst: &Path, ram: u32, fabric: bool, javacfg: &str, acct: Option<Acct>, iname: &str, sh: &Arc<Mutex<Shared>>) -> R<()> {
    let name = acct.as_ref().map_or("Player".to_string(), |a| a.name.clone());
    let uuid = acct.as_ref().map_or("00000000-0000-0000-0000-000000000000".to_string(), |a| a.uuid.clone());
    let token = acct.as_ref().map(|a| a.token.clone()).filter(|t| !t.is_empty()).unwrap_or("0".to_string());
    let utype = if acct.as_ref().map_or(false, |a| !a.token.is_empty()) { "msa" } else { "legacy" };
    let r = root();
    let vd = r.join("versions").join(id);
    set(sh, "Downloading version data..."); prog(sh, 0.03);
    let vjp = vd.join("version.json");
    let cached: Value = fs::read(&vjp).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let vj = if cached.is_null() { let v = json(url)?; let _ = fs::create_dir_all(&vd); let _ = fs::write(&vjp, v.to_string()); v } else { cached };
    let mut main = vj["mainClass"].as_str().unwrap().to_string();
    let mut fabs: Vec<String> = vec![];
    let mut fab_args = Value::Null;
    if fabric {
        set(sh, "Downloading Fabric...");
        let fpp = vd.join("fabric.json");
        let fp: Value = match fs::read(&fpp).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
            Some(v) => v,
            None => {
                let l = json(&format!("https://meta.fabricmc.net/v2/versions/loader/{id}"))?;
                let lv = l.get(0).and_then(|x| x["loader"]["version"].as_str()).ok_or("No Fabric for this version (1.14 or newer needed)")?.to_string();
                let v = json(&format!("https://meta.fabricmc.net/v2/versions/loader/{id}/{lv}/profile/json"))?;
                let _ = fs::create_dir_all(&vd); let _ = fs::write(&fpp, v.to_string());
                v
            }
        };
        main = fp["mainClass"].as_str().unwrap().to_string();
        fab_args = fp["arguments"].clone();
        for lib in fp["libraries"].as_array().unwrap() {
            let n = lib["name"].as_str().unwrap();
            let base = lib["url"].as_str().unwrap_or("https://repo1.maven.org/maven2/");
            let p = r.join("libraries").join(maven(n));
            dl(&format!("{base}{}", maven(n)), &p)?;
            fabs.push(p.to_string_lossy().into());
        }
    }
    let natives = vd.join("natives");
    fs::create_dir_all(&natives)?;
    let jar = vd.join(format!("{id}.jar"));
    set(sh, "Downloading game..."); prog(sh, 0.08);
    dl(vj["downloads"]["client"]["url"].as_str().unwrap(), &jar)?;

    set(sh, "Downloading libraries..."); prog(sh, 0.15);
    let mut cp: Vec<String> = vec![];
    let mut todo: Vec<(String, PathBuf)> = vec![];
    let mut nat: Vec<PathBuf> = vec![];
    for lib in vj["libraries"].as_array().unwrap() {
        if !allowed(lib.get("rules")) { continue; }
        if let Some(a) = lib["downloads"].get("artifact") {
            let p = r.join("libraries").join(a["path"].as_str().unwrap());
            todo.push((a["url"].as_str().unwrap().to_string(), p.clone()));
            cp.push(p.to_string_lossy().into());
        }
        if let Some(k) = lib["natives"]["windows"].as_str() {
            let k = k.replace("${arch}", "64");
            if let Some(c) = lib["downloads"]["classifiers"].get(&k) {
                let p = r.join("libraries").join(c["path"].as_str().unwrap());
                todo.push((c["url"].as_str().unwrap().to_string(), p.clone()));
                nat.push(p);
            }
        }
    }
    std::thread::scope(|s| {
        for ch in todo.chunks(todo.len() / 8 + 1) {
            s.spawn(move || for (u, p) in ch { if dl(u, p).is_err() { let _ = dl(u, p); } });
        }
    });
    let have_natives = fs::read_dir(&natives).map_or(false, |d| d.count() > 0);
    for p in nat.iter().filter(|_| !have_natives) {
        let mut z = zip::ZipArchive::new(fs::File::open(p)?)?;
        for i in 0..z.len() {
            let mut f = z.by_index(i)?;
            if f.is_dir() || f.name().starts_with("META-INF") { continue; }
            let out = natives.join(f.name());
            if let Some(d) = out.parent() { fs::create_dir_all(d)?; }
            std::io::copy(&mut f, &mut fs::File::create(out)?)?;
        }
    }
    cp.extend(fabs);
    cp.push(jar.to_string_lossy().into());

    set(sh, "Downloading assets..."); prog(sh, 0.25);
    let ai = &vj["assetIndex"];
    let aid = ai["id"].as_str().unwrap();
    let ip = r.join("assets/indexes").join(format!("{aid}.json"));
    dl(ai["url"].as_str().unwrap(), &ip)?;
    let idx: Value = serde_json::from_slice(&fs::read(&ip)?)?;
    let items: Vec<String> = idx["objects"].as_object().unwrap().values().map(|o| o["hash"].as_str().unwrap().to_string()).collect();
    let total = items.len().max(1);
    let done = Arc::new(Mutex::new(0usize));
    std::thread::scope(|s| {
        for ch in items.chunks(total / 16 + 1) {
            let (r, sh, done) = (&r, sh, done.clone());
            s.spawn(move || for h in ch {
                let p = r.join("assets/objects").join(&h[..2]).join(h);
                let _ = dl(&format!("https://resources.download.minecraft.net/{}/{}", &h[..2], h), &p);
                let mut d = done.lock().unwrap(); *d += 1;
                if *d % 40 == 0 { prog(sh, 0.25 + 0.6 * (*d as f32 / total as f32)); set(sh, &format!("Assets: {}/{}", *d, total)); }
            });
        }
    });

    set(sh, "Preparing Java..."); prog(sh, 0.9);
    let comp = vj["javaVersion"]["component"].as_str().unwrap_or("jre-legacy").to_string();
    let java: PathBuf = if javacfg.trim().is_empty() {
        ensure_java(&comp, sh).unwrap_or_else(|_| PathBuf::from("java"))
    } else { PathBuf::from(javacfg.trim()) };

    let game_dir = inst.to_path_buf();
    fs::create_dir_all(game_dir.join("mods"))?;
    let op = game_dir.join("options.txt");
    if !op.exists() {
        let _ = fs::write(&op, "renderDistance:8\nsimulationDistance:6\ngraphicsMode:0\nfancyGraphics:false\nmaxFps:260\nenableVsync:false\nparticles:2\nentityShadows:false\nbiomeBlendRadius:0\nmipmapLevels:0\nrenderClouds:\"false\"\nentityDistanceScaling:0.5\nao:false\n");
    }
    let cps = cp.join(";");
    let sub = |s: &str| -> String {
        [("${auth_player_name}", name.as_str()), ("${version_name}", id), ("${game_directory}", game_dir.to_str().unwrap()),
         ("${assets_root}", r.join("assets").to_str().unwrap()), ("${assets_index_name}", aid),
         ("${auth_uuid}", uuid.as_str()), ("${auth_access_token}", token.as_str()),
         ("${user_type}", utype), ("${version_type}", "Project Warden"), ("${natives_directory}", natives.to_str().unwrap()),
         ("${launcher_name}", "ProjectWarden"), ("${launcher_version}", "0.2"), ("${classpath}", &cps),
         ("${user_properties}", "{}"), ("${clientid}", "0"), ("${auth_xuid}", "0"),
         ("${game_assets}", r.join("assets").to_str().unwrap()),
         ("${library_directory}", r.join("libraries").to_str().unwrap()), ("${classpath_separator}", ";")]
            .iter().fold(s.to_string(), |a, (k, v)| a.replace(k, v))
    };
    let collect = |arr: &Value| -> Vec<String> {
        let mut o = vec![];
        for a in arr.as_array().into_iter().flatten() {
            if let Some(s) = a.as_str() { o.push(sub(s)); }
            else if allowed(a.get("rules")) && a.get("rules").map_or(true, |r| !r.to_string().contains("features")) {
                match &a["value"] {
                    Value::String(s) => o.push(sub(s)),
                    Value::Array(v) => for s in v { o.push(sub(s.as_str().unwrap())) },
                    _ => {}
                }
            }
        }
        o
    };
    let (jvm, game) = if vj.get("arguments").is_some() {
        let mut j = collect(&vj["arguments"]["jvm"]); j.extend(collect(&fab_args["jvm"]));
        let mut g = collect(&vj["arguments"]["game"]); g.extend(collect(&fab_args["game"]));
        (j, g)
    } else {
        (vec![sub("-Djava.library.path=${natives_directory}"), "-cp".into(), cps.clone()],
         vj["minecraftArguments"].as_str().unwrap().split(' ').map(&sub).collect())
    };
    set(sh, "Starting..."); prog(sh, 0.97);
    if let Some(j) = java.to_str() {
        let mut c = Command::new("reg");
        c.args(["add", r"HKCU\Software\Microsoft\DirectX\UserGpuPreferences", "/v", j, "/t", "REG_SZ", "/d", "GpuPreference=2;", "/f"]);
        #[cfg(windows)]
        { use std::os::windows::process::CommandExt; c.creation_flags(0x08000000); }
        let _ = c.output();
    }
    let mut cds: Vec<String> = vec![];
    if javacfg.trim().is_empty() && (comp == "java-runtime-delta" || comp == "java-runtime-epsilon") {
        let _ = fs::create_dir_all(r.join("cds"));
        cds = vec!["-XX:+AutoCreateSharedArchive".into(), format!("-XX:SharedArchiveFile={}", r.join("cds").join(format!("{id}-{}.jsa", fabric as u8)).display())];
    }
    let log = game_dir.join("launcher.log");
    let lf = fs::File::create(&log)?;
    let mut cmd = Command::new(&java);
    cmd.arg(format!("-Xmx{ram}M")).arg(format!("-Xms{}M", ram / 2))
        .args(["-XX:+UseG1GC", "-XX:+ParallelRefProcEnabled", "-XX:MaxGCPauseMillis=100", "-XX:+UnlockExperimentalVMOptions",
               "-XX:+DisableExplicitGC", "-XX:G1NewSizePercent=30", "-XX:G1MaxNewSizePercent=40", "-XX:G1HeapRegionSize=8M",
               "-XX:G1ReservePercent=20", "-XX:InitiatingHeapOccupancyPercent=15", "-XX:SurvivorRatio=32",
               "-XX:+PerfDisableSharedMem", "-XX:MaxTenuringThreshold=1"])
        .args(&cds).args(jvm).arg(&main).args(game).current_dir(&game_dir)
        .stdout(lf.try_clone()?).stderr(lf);
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000080); }
    let mut child = cmd.spawn().map_err(|_| "Could not start Java.")?;
    for _ in 0..16 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if let Ok(Some(st)) = child.try_wait() {
            if !st.success() {
                let t = fs::read_to_string(&log).unwrap_or_default();
                let tail: String = t.chars().rev().take(500).collect::<String>().chars().rev().collect();
                return Err(format!("The game stopped: {tail}").into());
            }
        }
    }
    prog(sh, 1.0);
    set(sh, "Minecraft started! Have fun!");
    sh.lock().unwrap().launched = true;
    { let sh3 = sh.clone(); let nm = iname.to_string();
      std::thread::spawn(move || { let t0 = std::time::Instant::now(); let _ = child.wait(); sh3.lock().unwrap().ended.push((nm, t0.elapsed().as_secs())); }); }
    Ok(())
}

const CLIENT_ID: &str = "15a646e3-ae9e-4dc2-8e3c-2b2feb5b1d0f";
const ORANGE: Color32 = Color32::from_rgb(255, 176, 32);
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";

#[derive(Clone, Default)]
struct Acct { name: String, uuid: String, token: String, refresh: String }
#[derive(Clone)]
struct Inst { name: String, ver: String, kind: u8, ram: u32, last: u64, played: u64, icon: u8, bg: u8 }

fn inst_path(i: &Inst) -> PathBuf { root().join("instances").join(i.name.replace(['/', '\\', ':'], "_")) }
fn load_insts() -> Vec<Inst> {
    fs::read_to_string(root().join("instances.txt")).unwrap_or_default().lines().filter_map(|l| {
        let p: Vec<&str> = l.split('|').collect();
        if p.len() < 4 { return None; }
        let kind: u8 = p[2].parse().ok()?;
        if kind > 1 { return None; }
        Some(Inst { name: p[0].into(), ver: p[1].into(), kind, ram: p[3].parse().ok()?,
            last: p.get(4).and_then(|x| x.parse().ok()).unwrap_or(0), played: p.get(5).and_then(|x| x.parse().ok()).unwrap_or(0),
            icon: p.get(6).and_then(|x| x.parse().ok()).unwrap_or(kind), bg: p.get(7).and_then(|x| x.parse().ok()).unwrap_or((p[0].len() % 8) as u8) })
    }).collect()
}
fn save_insts(v: &[Inst]) {
    let _ = fs::create_dir_all(root());
    let t: Vec<String> = v.iter().map(|i| format!("{}|{}|{}|{}|{}|{}|{}|{}", i.name.replace('|', "_"), i.ver, i.kind, i.ram, i.last, i.played, i.icon, i.bg)).collect();
    let _ = fs::write(root().join("instances.txt"), t.join("\n"));
}
fn save_acct(a: &Acct) {
    let _ = fs::create_dir_all(root());
    let _ = fs::write(root().join("account.txt"), format!("{}\n{}\n{}", a.name, a.uuid, a.refresh));
}
fn load_acct() -> Option<Acct> {
    let t = fs::read_to_string(root().join("account.txt")).ok()?;
    let l: Vec<&str> = t.lines().collect();
    if l.len() < 3 { return None; }
    Some(Acct { name: l[0].into(), uuid: l[1].into(), token: String::new(), refresh: l[2].into() })
}
fn load_head(sh: &Arc<Mutex<Shared>>, uuid: String) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        if let Ok(ci) = fetch_icon(&format!("https://mc-heads.net/avatar/{uuid}/64")) { sh.lock().unwrap().head = Some(ci); }
        if let Ok(b) = get(&format!("https://mc-heads.net/skin/{uuid}")) {
            if let Ok(im) = image::load_from_memory(&b) { sh.lock().unwrap().acct_skin = Some(norm_skin(im.to_rgba8())); }
        }
    });
}
fn load_guest(sh: &Arc<Mutex<Shared>>) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        if let Ok(b) = get("https://s.namemc.com/i/d999297e634bcc3d.png") {
            if let Ok(im) = image::load_from_memory(&b) { sh.lock().unwrap().guest_full = Some(norm_skin(im.to_rgba8())); }
        }
        for n in ["Steve", "Alex"] {
            if let Ok(b) = get(&format!("https://mc-heads.net/skin/MHF_{n}")) {
                if let Ok(im) = image::load_from_memory(&b) { let (i, sl) = norm_skin(im.to_rgba8()); sh.lock().unwrap().defaults.push((n.to_string(), i, sl)); }
            }
        }
    });
}
fn post_json(url: &str, body: &Value) -> R<Value> { Ok(client().post(url).json(body).send()?.error_for_status()?.json()?) }
fn mc_chain(ms: &str, refresh: String) -> R<Acct> {
    let x = post_json("https://user.auth.xboxlive.com/user/authenticate", &serde_json::json!({
        "Properties": {"AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": format!("d={ms}")},
        "RelyingParty": "http://auth.xboxlive.com", "TokenType": "JWT"}))?;
    let xt = x["Token"].as_str().ok_or("Xbox hiba")?.to_string();
    let uhs = x["DisplayClaims"]["xui"][0]["uhs"].as_str().ok_or("Xbox hiba")?.to_string();
    let s = post_json("https://xsts.auth.xboxlive.com/xsts/authorize", &serde_json::json!({
        "Properties": {"SandboxId": "RETAIL", "UserTokens": [xt]},
        "RelyingParty": "rp://api.minecraftservices.com/", "TokenType": "JWT"}))?;
    let st = s["Token"].as_str().ok_or("Xbox hiba")?.to_string();
    let m: Value = client().post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({"identityToken": format!("XBL3.0 x={uhs};{st}")})).send()?.error_for_status()
        .map_err(|_| "Mojang has not approved this app ID yet. Use the Username login for now")?.json()?;
    let tok = m["access_token"].as_str().ok_or("nincs Minecraft token")?.to_string();
    let p: Value = client().get("https://api.minecraftservices.com/minecraft/profile").bearer_auth(&tok).send()?.error_for_status()
        .map_err(|_| "This account does not own Minecraft Java Edition")?.json()?;
    Ok(Acct { name: p["name"].as_str().unwrap_or("Player").into(), uuid: p["id"].as_str().unwrap_or("").into(), token: tok, refresh })
}
fn refresh_acct(cid: &str, refresh: &str) -> R<Acct> {
    let j: Value = client().post(TOKEN_URL).form(&[("client_id", cid), ("grant_type", "refresh_token"), ("refresh_token", refresh),
        ("scope", "XboxLive.signin offline_access")]).send()?.error_for_status()?.json()?;
    let at = j["access_token"].as_str().ok_or("nincs token")?;
    mc_chain(at, j["refresh_token"].as_str().unwrap_or(refresh).to_string())
}
fn spawn_login(sh: &Arc<Mutex<Shared>>, cid: String) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        let r = (|| -> R<()> {
            let d: Value = client().post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
                .form(&[("client_id", cid.as_str()), ("scope", "XboxLive.signin offline_access")]).send()?.error_for_status()?.json()?;
            let code = d["user_code"].as_str().ok_or("nincs kód")?.to_string();
            let uri = d["verification_uri"].as_str().unwrap_or("https://microsoft.com/link").to_string();
            let dc = d["device_code"].as_str().ok_or("nincs eszközkód")?.to_string();
            let iv = d["interval"].as_u64().unwrap_or(5);
            sh.lock().unwrap().login = format!("A web page opened. Enter this code: {code}");
            let _ = Command::new("cmd").args(["/C", "start", "", uri.as_str()]).spawn();
            for _ in 0..120 {
                std::thread::sleep(std::time::Duration::from_secs(iv));
                let j: Value = client().post(TOKEN_URL).form(&[("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", cid.as_str()), ("device_code", dc.as_str())]).send()?.json()?;
                if let Some(at) = j["access_token"].as_str() {
                    let a = mc_chain(at, j["refresh_token"].as_str().unwrap_or("").to_string())?;
                    save_acct(&a);
                    load_head(&sh, a.uuid.clone());
                    let mut s = sh.lock().unwrap();
                    s.acct = Some(a);
                    s.login.clear();
                    return Ok(());
                }
                match j["error"].as_str() { Some("authorization_pending") | Some("slow_down") => {}, _ => return Err("login cancelled".into()) }
            }
            Err("timed out".into())
        })();
        if let Err(e) = r { sh.lock().unwrap().login = format!("Error: {e}"); }
    });
}
fn current_acct(sh: &Arc<Mutex<Shared>>, cid: &str) -> Option<Acct> {
    let a = sh.lock().unwrap().acct.clone()?;
    if a.refresh == "offline" { return Some(a); }
    if a.refresh.is_empty() || cid.is_empty() { return None; }
    match refresh_acct(cid, &a.refresh) {
        Ok(n) => { save_acct(&n); sh.lock().unwrap().acct = Some(n.clone()); Some(n) }
        Err(e) => { set(sh, &format!("Login expired, please log in again: {e}")); None }
    }
}
fn play(sh: &Arc<Mutex<Shared>>, vers: &[(String, String)], i: &Inst, java: String, cid: String) {
    let Some((_, url)) = vers.iter().find(|v| v.0 == i.ver).cloned() else { set(sh, "This version was not found."); return };
    let (sh2, i2) = (sh.clone(), i.clone());
    { let mut s = sh.lock().unwrap(); s.busy = true; s.progress = 0.0; }
    std::thread::spawn(move || {
        let acct = current_acct(&sh2, &cid);
        if let Err(e) = launch(&i2.ver, &url, &inst_path(&i2), i2.ram, i2.kind == 1, &java, acct, &i2.name, &sh2) { set(&sh2, &format!("Error: {e}")); }
        sh2.lock().unwrap().busy = false;
    });
}

// ===================== NEW UI =====================
use std::sync::atomic::{AtomicU8, Ordering};
static LANG: AtomicU8 = AtomicU8::new(0);
const KEYS: &[&str] = &["Home","Skins","Discover content","Create instance","Settings","Logout","Playing as","Guest","Login","Microsoft login","Play","Jump in","Welcome back,","Open","Appearance","Features","Behavior","Language","Color theme","Dark","Light","Sync with system","Instances","Sidebar","Startup and navigation","Home and content","Minimize app","Compact mode","Show play time","Hide nametag","Skin selector","Saved skins","Default skins","Capes","Add skin","Add cape","No cape","Drag to rotate","Install","Cancel","Install project","Search","Modpacks","Mods","Resource Packs","Shaders","Create","Delete","Sync features across instances"];
const SETS: &[(&str, &[&str])] = &[
    ("hu", &["Kezdőlap","Skinek","Tartalmak keresése","Új példány","Beállítások","Kijelentkezés","Játékosként","Vendég","Belépés","Microsoft belépés","Játék","Folytasd","Üdv újra,","Megnyitás","Megjelenés","Funkciók","Működés","Nyelv","Színtéma","Sötét","Világos","Rendszer szerint","Példányok","Oldalsáv","Indítás és navigáció","Főoldal és tartalom","Program kicsinyítése","Kompakt mód","Játékidő mutatása","Névtábla elrejtése","Skinválasztó","Mentett skinek","Alap skinek","Köpenyek","Skin hozzáadása","Köpeny hozzáadása","Nincs köpeny","Húzd a forgatáshoz","Telepítés","Mégse","Projekt telepítése","Keresés","Modpackek","Modok","Textúracsomagok","Shaderek","Létrehozás","Törlés","Beállítások szinkronizálása példányok között"]),
    ("it", &["Home","Skin","Scopri contenuti","Crea istanza","Impostazioni","Esci","Giochi come","Ospite","Accedi","Accesso Microsoft","Gioca","Riprendi","Bentornato,","Apri","Aspetto","Funzionalità","Comportamento","Lingua","Tema colore","Scuro","Chiaro","Come il sistema","Istanze","Barra laterale","Avvio e navigazione","Home e contenuti","Riduci a icona l'app","Modalità compatta","Mostra tempo di gioco","Nascondi nametag","Selettore skin","Skin salvate","Skin predefinite","Mantelli","Aggiungi skin","Aggiungi mantello","Nessun mantello","Trascina per ruotare","Installa","Annulla","Installa progetto","Cerca","Modpack","Mod","Resource pack","Shader","Crea","Elimina","Sincronizza le impostazioni tra istanze"]),
    ("pt", &["Início","Skins","Descobrir conteúdo","Criar instância","Definições","Terminar sessão","A jogar como","Convidado","Entrar","Login Microsoft","Jogar","Continuar","Bem-vindo de volta,","Abrir","Aparência","Funcionalidades","Comportamento","Idioma","Tema de cores","Escuro","Claro","Igual ao sistema","Instâncias","Barra lateral","Arranque e navegação","Início e conteúdo","Minimizar a aplicação","Modo compacto","Mostrar tempo de jogo","Ocultar nome","Seletor de skins","Skins guardadas","Skins predefinidas","Capas","Adicionar skin","Adicionar capa","Sem capa","Arrasta para rodar","Instalar","Cancelar","Instalar projeto","Pesquisar","Modpacks","Mods","Pacotes de recursos","Shaders","Criar","Eliminar","Sincronizar definições entre instâncias"]),
    ("de", &["Startseite","Skins","Inhalte entdecken","Instanz erstellen","Einstellungen","Abmelden","Spielen als","Gast","Anmelden","Microsoft-Anmeldung","Spielen","Weiterspielen","Willkommen zurück,","Öffnen","Darstellung","Funktionen","Verhalten","Sprache","Farbschema","Dunkel","Hell","Wie System","Instanzen","Seitenleiste","Start und Navigation","Startseite und Inhalte","App minimieren","Kompaktmodus","Spielzeit anzeigen","Namensschild ausblenden","Skin-Auswahl","Gespeicherte Skins","Standard-Skins","Umhänge","Skin hinzufügen","Umhang hinzufügen","Kein Umhang","Zum Drehen ziehen","Installieren","Abbrechen","Projekt installieren","Suchen","Modpacks","Mods","Ressourcenpakete","Shader","Erstellen","Löschen","Einstellungen zwischen Instanzen synchronisieren"]),
    ("ru", &["Главная","Скины","Обзор контента","Создать сборку","Настройки","Выйти","Играете как","Гость","Войти","Вход через Microsoft","Играть","Продолжить","С возвращением,","Открыть","Внешний вид","Функции","Поведение","Язык","Цветовая тема","Тёмная","Светлая","Как в системе","Сборки","Боковая панель","Запуск и навигация","Главная и контент","Сворачивать приложение","Компактный режим","Показывать время игры","Скрыть ник","Выбор скина","Сохранённые скины","Стандартные скины","Плащи","Добавить скин","Добавить плащ","Без плаща","Перетащите для вращения","Установить","Отмена","Установка проекта","Поиск","Сборки модов","Моды","Наборы ресурсов","Шейдеры","Создать","Удалить","Синхронизировать настройки между сборками"]),
    ("es", &["Inicio","Skins","Descubrir contenido","Crear instancia","Ajustes","Cerrar sesión","Jugando como","Invitado","Iniciar sesión","Inicio de sesión Microsoft","Jugar","Continuar","Bienvenido de nuevo,","Abrir","Apariencia","Funciones","Comportamiento","Idioma","Tema de color","Oscuro","Claro","Igual que el sistema","Instancias","Barra lateral","Inicio y navegación","Inicio y contenido","Minimizar la aplicación","Modo compacto","Mostrar tiempo de juego","Ocultar nombre","Selector de skins","Skins guardadas","Skins predeterminadas","Capas","Añadir skin","Añadir capa","Sin capa","Arrastra para girar","Instalar","Cancelar","Instalar proyecto","Buscar","Modpacks","Mods","Paquetes de recursos","Shaders","Crear","Eliminar","Sincronizar ajustes entre instancias"]),
    ("pl", &["Strona główna","Skórki","Odkryj zawartość","Utwórz instancję","Ustawienia","Wyloguj","Grasz jako","Gość","Zaloguj","Logowanie Microsoft","Graj","Kontynuuj","Witaj z powrotem,","Otwórz","Wygląd","Funkcje","Zachowanie","Język","Motyw kolorystyczny","Ciemny","Jasny","Jak w systemie","Instancje","Pasek boczny","Uruchamianie i nawigacja","Strona główna i zawartość","Minimalizuj aplikację","Tryb kompaktowy","Pokaż czas gry","Ukryj nick","Wybór skórki","Zapisane skórki","Domyślne skórki","Peleryny","Dodaj skórkę","Dodaj pelerynę","Brak peleryny","Przeciągnij, aby obrócić","Zainstaluj","Anuluj","Instalacja projektu","Szukaj","Paczki modów","Mody","Paczki zasobów","Shadery","Utwórz","Usuń","Synchronizuj ustawienia między instancjami"]),
    ("uk", &["Головна","Скіни","Огляд вмісту","Створити збірку","Налаштування","Вийти","Граєте як","Гість","Увійти","Вхід через Microsoft","Грати","Продовжити","З поверненням,","Відкрити","Вигляд","Функції","Поведінка","Мова","Колірна тема","Темна","Світла","Як у системі","Збірки","Бічна панель","Запуск і навігація","Головна і вміст","Згортати застосунок","Компактний режим","Показувати час гри","Сховати нік","Вибір скіна","Збережені скіни","Стандартні скіни","Плащі","Додати скін","Додати плащ","Без плаща","Перетягніть для обертання","Встановити","Скасувати","Встановлення проєкту","Пошук","Збірки модів","Моди","Набори ресурсів","Шейдери","Створити","Видалити","Синхронізувати налаштування між збірками"]),
    ("fr", &["Accueil","Skins","Découvrir du contenu","Créer une instance","Paramètres","Déconnexion","Joue en tant que","Invité","Connexion","Connexion Microsoft","Jouer","Reprendre","Content de vous revoir,","Ouvrir","Apparence","Fonctionnalités","Comportement","Langue","Thème de couleur","Sombre","Clair","Comme le système","Instances","Barre latérale","Démarrage et navigation","Accueil et contenu","Réduire l'application","Mode compact","Afficher le temps de jeu","Masquer le pseudo","Sélecteur de skin","Skins enregistrés","Skins par défaut","Capes","Ajouter un skin","Ajouter une cape","Aucune cape","Glisser pour tourner","Installer","Annuler","Installer le projet","Rechercher","Modpacks","Mods","Packs de ressources","Shaders","Créer","Supprimer","Synchroniser les paramètres entre instances"]),
    ("nl", &["Home","Skins","Content ontdekken","Instance maken","Instellingen","Uitloggen","Speelt als","Gast","Inloggen","Microsoft-login","Spelen","Verdergaan","Welkom terug,","Openen","Uiterlijk","Functies","Gedrag","Taal","Kleurthema","Donker","Licht","Zoals systeem","Instances","Zijbalk","Opstarten en navigatie","Home en content","App minimaliseren","Compacte modus","Speeltijd tonen","Naamplaatje verbergen","Skinkiezer","Opgeslagen skins","Standaardskins","Capes","Skin toevoegen","Cape toevoegen","Geen cape","Sleep om te draaien","Installeren","Annuleren","Project installeren","Zoeken","Modpacks","Mods","Resourcepacks","Shaders","Maken","Verwijderen","Instellingen synchroniseren tussen instances"]),
    ("sv", &["Hem","Skinn","Upptäck innehåll","Skapa instans","Inställningar","Logga ut","Spelar som","Gäst","Logga in","Microsoft-inloggning","Spela","Fortsätt","Välkommen tillbaka,","Öppna","Utseende","Funktioner","Beteende","Språk","Färgtema","Mörkt","Ljust","Som systemet","Instanser","Sidofält","Start och navigering","Hem och innehåll","Minimera appen","Kompakt läge","Visa speltid","Dölj namnskylt","Skinnväljare","Sparade skinn","Standardskinn","Mantlar","Lägg till skinn","Lägg till mantel","Ingen mantel","Dra för att rotera","Installera","Avbryt","Installera projekt","Sök","Modpack","Mods","Resurspaket","Shaders","Skapa","Ta bort","Synka inställningar mellan instanser"]),
    ("tr", &["Ana sayfa","Skinler","İçerik keşfet","Örnek oluştur","Ayarlar","Çıkış yap","Şu hesapla oynuyor","Misafir","Giriş yap","Microsoft girişi","Oyna","Devam et","Tekrar hoş geldin,","Aç","Görünüm","Özellikler","Davranış","Dil","Renk teması","Koyu","Açık","Sistem gibi","Örnekler","Kenar çubuğu","Başlangıç ve gezinme","Ana sayfa ve içerik","Uygulamayı küçült","Kompakt mod","Oyun süresini göster","Ad etiketini gizle","Skin seçici","Kayıtlı skinler","Varsayılan skinler","Pelerinler","Skin ekle","Pelerin ekle","Pelerin yok","Döndürmek için sürükle","Yükle","İptal","Projeyi yükle","Ara","Mod paketleri","Modlar","Kaynak paketleri","Gölgelendiriciler","Oluştur","Sil","Ayarları örnekler arasında eşitle"]),
    ("cs", &["Domů","Skiny","Objevovat obsah","Vytvořit instanci","Nastavení","Odhlásit se","Hrajete jako","Host","Přihlásit se","Přihlášení Microsoft","Hrát","Pokračovat","Vítejte zpět,","Otevřít","Vzhled","Funkce","Chování","Jazyk","Barevný motiv","Tmavý","Světlý","Podle systému","Instance","Postranní panel","Spuštění a navigace","Domů a obsah","Minimalizovat aplikaci","Kompaktní režim","Zobrazit herní čas","Skrýt jmenovku","Výběr skinu","Uložené skiny","Výchozí skiny","Pláště","Přidat skin","Přidat plášť","Bez pláště","Přetažením otočíte","Instalovat","Zrušit","Instalovat projekt","Hledat","Modpacky","Módy","Balíčky zdrojů","Shadery","Vytvořit","Smazat","Synchronizovat nastavení mezi instancemi"]),
    ("sr", &["Početna","Skinovi","Istraži sadržaj","Napravi instancu","Podešavanja","Odjavi se","Igraš kao","Gost","Prijavi se","Microsoft prijava","Igraj","Nastavi","Dobrodošao nazad,","Otvori","Izgled","Funkcije","Ponašanje","Jezik","Tema boja","Tamna","Svetla","Prema sistemu","Instance","Bočna traka","Pokretanje i navigacija","Početna i sadržaj","Minimizuj aplikaciju","Kompaktni režim","Prikaži vreme igranja","Sakrij oznaku imena","Izbor skina","Sačuvani skinovi","Podrazumevani skinovi","Plaštevi","Dodaj skin","Dodaj plašt","Bez plašta","Prevuci za rotaciju","Instaliraj","Otkaži","Instaliraj projekat","Pretraži","Modpakovi","Modovi","Paketi resursa","Šejderi","Napravi","Obriši","Sinhronizuj podešavanja između instanci"]),
    ("ro", &["Acasă","Skinuri","Descoperă conținut","Creează instanță","Setări","Deconectare","Joci ca","Invitat","Autentificare","Autentificare Microsoft","Joacă","Continuă","Bine ai revenit,","Deschide","Aspect","Funcții","Comportament","Limbă","Temă de culoare","Întunecat","Luminos","Ca sistemul","Instanțe","Bară laterală","Pornire și navigare","Acasă și conținut","Minimizează aplicația","Mod compact","Arată timpul de joc","Ascunde numele","Selector de skinuri","Skinuri salvate","Skinuri implicite","Pelerine","Adaugă skin","Adaugă pelerină","Fără pelerină","Trage pentru a roti","Instalează","Anulează","Instalează proiectul","Caută","Modpackuri","Moduri","Pachete de resurse","Shadere","Creează","Șterge","Sincronizează setările între instanțe"]),
    ("nb", &["Hjem","Skins","Utforsk innhold","Opprett instans","Innstillinger","Logg ut","Spiller som","Gjest","Logg inn","Microsoft-innlogging","Spill","Fortsett","Velkommen tilbake,","Åpne","Utseende","Funksjoner","Oppførsel","Språk","Fargetema","Mørk","Lys","Følg systemet","Instanser","Sidefelt","Oppstart og navigasjon","Hjem og innhold","Minimer appen","Kompakt modus","Vis spilletid","Skjul navneskilt","Skinnvelger","Lagrede skins","Standard skins","Kapper","Legg til skin","Legg til kappe","Ingen kappe","Dra for å rotere","Installer","Avbryt","Installer prosjekt","Søk","Modpakker","Moduser","Ressurspakker","Shaders","Opprett","Slett","Synkroniser innstillinger mellom instanser"]),
    ("da", &["Hjem","Skins","Find indhold","Opret instans","Indstillinger","Log ud","Spiller som","Gæst","Log ind","Microsoft-login","Spil","Fortsæt","Velkommen tilbage,","Åbn","Udseende","Funktioner","Adfærd","Sprog","Farvetema","Mørk","Lys","Følg systemet","Instanser","Sidepanel","Start og navigation","Hjem og indhold","Minimer appen","Kompakt tilstand","Vis spilletid","Skjul navneskilt","Skinvælger","Gemte skins","Standardskins","Kapper","Tilføj skin","Tilføj kappe","Ingen kappe","Træk for at dreje","Installer","Annuller","Installer projekt","Søg","Modpacks","Mods","Ressourcepakker","Shaders","Opret","Slet","Synkroniser indstillinger mellem instanser"]),
    ("fi", &["Koti","Skinit","Löydä sisältöä","Luo instanssi","Asetukset","Kirjaudu ulos","Pelaat nimellä","Vieras","Kirjaudu","Microsoft-kirjautuminen","Pelaa","Jatka","Tervetuloa takaisin,","Avaa","Ulkoasu","Ominaisuudet","Toiminta","Kieli","Väriteema","Tumma","Vaalea","Kuten järjestelmä","Instanssit","Sivupalkki","Käynnistys ja navigointi","Koti ja sisältö","Pienennä sovellus","Kompakti tila","Näytä peliaika","Piilota nimikyltti","Skinivalitsin","Tallennetut skinit","Oletusskinit","Viitat","Lisää skini","Lisää viitta","Ei viittaa","Käännä vetämällä","Asenna","Peruuta","Asenna projekti","Hae","Modipaketit","Modit","Resurssipaketit","Varjostimet","Luo","Poista","Synkronoi asetukset instanssien välillä"]),
];
fn tr(s: &'static str) -> &'static str {
    let l = LANG.load(Ordering::Relaxed) as usize;
    if l == 0 || l > SETS.len() { return s; }
    match KEYS.iter().position(|k| *k == s) { Some(i) => SETS[l - 1].1.get(i).copied().unwrap_or(s), None => s }
}
fn lang_idx(code: &str) -> u8 {
    let c = match code { "pt-BR" => "pt", "de-CH" => "de", "es-419" => "es", x => x };
    SETS.iter().position(|s| s.0 == c).map(|i| (i + 1) as u8).unwrap_or(0)
}

fn c3(r: u8, g: u8, b: u8) -> Color32 { Color32::from_rgb(r, g, b) }
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}
fn now_secs() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn fmt_time(s: u64) -> String { let m = s / 60; if m >= 60 { format!("{}h {}m", m / 60, m % 60) } else { format!("{}m", m) } }
fn system_light() -> bool {
    let mut c = Command::new("reg");
    c.args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "/v", "AppsUseLightTheme"]);
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x08000000); }
    c.output().map(|o| String::from_utf8_lossy(&o.stdout).contains("0x1")).unwrap_or(false)
}

// ---------- palette ----------
#[derive(Clone, Copy)]
struct Pal { side: Color32, bg: Color32, card: Color32, stroke: Color32, input: Color32, hover: Color32, light: bool }
fn pal_for(mode: u8) -> Pal {
    match mode {
        1 => Pal { side: c3(250, 250, 252), bg: c3(238, 240, 244), card: c3(255, 255, 255), stroke: c3(216, 220, 228), input: c3(246, 247, 250), hover: c3(228, 231, 237), light: true },
        2 => Pal { side: c3(0, 0, 0), bg: c3(0, 0, 0), card: c3(15, 15, 18), stroke: c3(36, 37, 42), input: c3(9, 9, 11), hover: c3(28, 29, 34), light: false },
        _ => Pal { side: c3(17, 18, 22), bg: c3(22, 24, 28), card: c3(31, 34, 40), stroke: c3(46, 50, 58), input: c3(14, 15, 18), hover: c3(40, 44, 52), light: false },
    }
}
fn apply_visuals(ctx: &egui::Context, p: &Pal) {
    let mut v = if p.light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = p.bg; v.window_fill = p.card; v.extreme_bg_color = p.input;
    v.selection.bg_fill = ACCENT; v.selection.stroke = egui::Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.window_stroke = egui::Stroke::new(1.0, p.stroke);
    v.widgets.inactive.weak_bg_fill = p.hover; v.widgets.inactive.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.stroke; v.widgets.hovered.bg_fill = p.stroke;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = egui::Rounding::same(6.0);
    }
    ctx.set_visuals(v);
}

// ---------- settings ----------
#[derive(Clone)]
struct Cfg {
    theme: u8, lang: String, sync: bool, worlds: bool, files: bool, shots: bool, side_shots: bool, side_skins: bool,
    quick: u32, minimize: bool, refocus: bool, hide_right: bool, compact: bool, playtime: bool, hide_tag: bool,
    skin: String, cape: String, java: String,
    cloud_on: bool, cloud_url: String, cloud_pw: String,
}
impl Cfg {
    fn new() -> Self {
        Cfg { theme: 0, lang: "en".into(), sync: false, worlds: true, files: true, shots: false, side_shots: true, side_skins: true,
              quick: 5, minimize: false, refocus: false, hide_right: false, compact: false, playtime: true, hide_tag: false,
              skin: String::new(), cape: String::new(), java: String::new(), cloud_on: false, cloud_url: String::new(), cloud_pw: String::new() }
    }
}
fn read_cfg() -> Cfg {
    let mut c = Cfg::new();
    let t = fs::read_to_string(root().join("config.txt")).unwrap_or_default();
    for l in t.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let b = v == "1";
        match k {
            "theme" => c.theme = v.parse().unwrap_or(0), "lang" => c.lang = v.into(), "sync" => c.sync = b,
            "worlds" => c.worlds = b, "files" => c.files = b, "shots" => c.shots = b, "side_shots" => c.side_shots = b,
            "side_skins" => c.side_skins = b, "quick" => c.quick = v.parse().unwrap_or(5), "minimize" => c.minimize = b,
            "refocus" => c.refocus = b, "hide_right" => c.hide_right = b, "compact" => c.compact = b,
            "playtime" => c.playtime = b, "hide_tag" => c.hide_tag = b, "skin" => c.skin = v.into(),
            "cape" => c.cape = v.into(), "java" => c.java = v.into(),
            "cloud_on" => c.cloud_on = b, "cloud_url" => c.cloud_url = v.into(), "cloud_pw" => c.cloud_pw = v.into(),
            _ => {}
        }
    }
    c
}
fn write_cfg(c: &Cfg) {
    let b = |x: bool| if x { "1" } else { "0" };
    let t = format!("theme={}\nlang={}\nsync={}\nworlds={}\nfiles={}\nshots={}\nside_shots={}\nside_skins={}\nquick={}\nminimize={}\nrefocus={}\nhide_right={}\ncompact={}\nplaytime={}\nhide_tag={}\nskin={}\ncape={}\njava={}\ncloud_on={}\ncloud_url={}\ncloud_pw={}\n",
        c.theme, c.lang, b(c.sync), b(c.worlds), b(c.files), b(c.shots), b(c.side_shots), b(c.side_skins), c.quick, b(c.minimize),
        b(c.refocus), b(c.hide_right), b(c.compact), b(c.playtime), b(c.hide_tag), c.skin, c.cape, c.java, b(c.cloud_on), c.cloud_url, c.cloud_pw);
    let _ = fs::create_dir_all(root());
    let _ = fs::write(root().join("config.txt"), t);
}

// code, native name (empty = not shown), english name, percent translated
const LANGS: &[(&str, &str, &str, u32)] = &[
    ("en", "", "English (United States)", 100), ("it", "Italiano (Italia)", "Italian (Italy)", 84), ("ko", "", "Korean", 83),
    ("pt-BR", "Português (Brasil)", "Portuguese (Brazil)", 83), ("de-CH", "Deutsch (Schweiz)", "German (Switzerland)", 82),
    ("de", "Deutsch (Deutschland)", "German (Germany)", 82), ("hu", "Magyar (Magyarország)", "Hungarian", 82),
    ("ru", "Русский", "Russian", 82), ("zh-CN", "", "Chinese (Simplified)", 82), ("zh-TW", "", "Chinese (Traditional)", 82),
    ("es-419", "Español (Latinoamérica)", "Spanish (Latin America)", 80), ("pl", "Polski", "Polish", 80),
    ("uk", "Українська", "Ukrainian", 80), ("fr", "Français", "French", 78), ("vi", "Tiếng Việt", "Vietnamese", 77),
    ("es", "Español (España)", "Spanish (Spain)", 76), ("nl", "Nederlands", "Dutch", 76), ("sv", "Svenska", "Swedish", 73),
    ("tr", "Türkçe", "Turkish", 73), ("ja", "", "Japanese", 65), ("cs", "Čeština", "Czech", 59),
    ("sr", "Srpski (Latinica)", "Serbian (Latin)", 50), ("ms", "Bahasa Melayu", "Malay", 46),
    ("pt", "Português (Portugal)", "Portuguese (Portugal)", 32), ("ro", "Română (România)", "Romanian", 32),
    ("id", "Bahasa Indonesia", "Indonesian", 27), ("nb", "Norsk (Bokmål)", "Norwegian Bokmål", 24), ("da", "Dansk", "Danish", 23),
    ("fil", "Filipino", "Filipino", 23), ("he", "", "Hebrew", 21), ("fi", "Suomi", "Finnish", 18), ("th", "", "Thai", 14),
];

// ---------- skins: normalise + 3D preview ----------
fn norm_skin(mut im: image::RgbaImage) -> (image::RgbaImage, bool) {
    let (w, h) = im.dimensions();
    if w != 64 { let nh = if h * 2 == w { 32 } else { 64 }; im = image::imageops::resize(&im, 64, nh, image::imageops::FilterType::Nearest); }
    if im.height() == 32 {
        let mut n = image::RgbaImage::new(64, 64);
        image::imageops::replace(&mut n, &im, 0, 0);
        for y in 0..16 { for x in 0..16 {
            let a = *im.get_pixel(x, 16 + y); n.put_pixel(16 + x, 48 + y, a);
            let b = *im.get_pixel(40 + x, 16 + y); n.put_pixel(32 + x, 48 + y, b);
        } }
        im = n;
    }
    let slim = im.get_pixel(55, 20)[3] == 0;
    for (x0, y0, w, h) in [(0u32, 0u32, 32u32, 16u32), (16, 16, 24, 16), (40, 16, 16, 16), (0, 16, 16, 16), (16, 48, 16, 16), (32, 48, 16, 16)] {
        for y in y0..y0 + h { for x in x0..x0 + w { im.get_pixel_mut(x, y)[3] = 255; } }
    }
    (im, slim)
}
fn cape_image(mut im: image::RgbaImage) -> egui::ColorImage {
    if im.width() % 64 != 0 || im.width() / 64 * 32 != im.height() { im = image::imageops::resize(&im, 64, 32, image::imageops::FilterType::Nearest); }
    egui::ColorImage::from_rgba_unmultiplied([im.width() as usize, im.height() as usize], im.as_raw())
}
struct SkinEntry { name: String, tex: egui::TextureHandle, slim: bool }
struct CapeEntry { name: String, tex: egui::TextureHandle }
fn mk_skin(ctx: &egui::Context, name: &str, im: &image::RgbaImage, slim: bool) -> SkinEntry {
    let ci = egui::ColorImage::from_rgba_unmultiplied([64, 64], im.as_raw());
    SkinEntry { name: name.to_string(), tex: ctx.load_texture(format!("sk_{name}"), ci, egui::TextureOptions::NEAREST), slim }
}

#[derive(Clone, Copy)]
struct V3 { x: f32, y: f32, z: f32 }
struct Face { c: [V3; 4], uv: [f32; 4], tex: u8, part: u8 }
fn bx(f: &mut Vec<Face>, a: V3, b: V3, u: f32, v: f32, w: f32, h: f32, d: f32, t: u8, sw: bool, part: u8) {
    let (x0, y0, z0, x1, y1, z1) = (a.x, a.y, a.z, b.x, b.y, b.z);
    let p = |x: f32, y: f32, z: f32| V3 { x, y, z };
    let (fu, bu) = if sw { (u + 2.0 * d + w, u + d) } else { (u + d, u + 2.0 * d + w) };
    f.push(Face { c: [p(x0, y1, z1), p(x1, y1, z1), p(x1, y0, z1), p(x0, y0, z1)], uv: [fu, v + d, w, h], tex: t, part });
    f.push(Face { c: [p(x1, y1, z0), p(x0, y1, z0), p(x0, y0, z0), p(x1, y0, z0)], uv: [bu, v + d, w, h], tex: t, part });
    f.push(Face { c: [p(x0, y1, z0), p(x0, y1, z1), p(x0, y0, z1), p(x0, y0, z0)], uv: [u, v + d, d, h], tex: t, part });
    f.push(Face { c: [p(x1, y1, z1), p(x1, y1, z0), p(x1, y0, z0), p(x1, y0, z1)], uv: [u + d + w, v + d, d, h], tex: t, part });
    f.push(Face { c: [p(x0, y1, z0), p(x1, y1, z0), p(x1, y1, z1), p(x0, y1, z1)], uv: [u + d, v, w, d], tex: t, part });
}
fn model(slim: bool, cape: bool) -> Vec<Face> {
    let mut f = vec![];
    let aw = if slim { 3.0 } else { 4.0 };
    let v = |x: f32, y: f32, z: f32| V3 { x, y, z };
    let (g, k) = (0.5, 0.25);
    bx(&mut f, v(-4., 24., -4.), v(4., 32., 4.), 0., 0., 8., 8., 8., 0, false, 0);
    bx(&mut f, v(-4. - g, 24. - g, -4. - g), v(4. + g, 32. + g, 4. + g), 32., 0., 8., 8., 8., 0, false, 0);
    bx(&mut f, v(-4., 12., -2.), v(4., 24., 2.), 16., 16., 8., 12., 4., 0, false, 1);
    bx(&mut f, v(-4. - k, 12. - k, -2. - k), v(4. + k, 24. + k, 2. + k), 16., 32., 8., 12., 4., 0, false, 1);
    bx(&mut f, v(-4. - aw, 12., -2.), v(-4., 24., 2.), 40., 16., aw, 12., 4., 0, false, 2);
    bx(&mut f, v(-4. - aw - k, 12. - k, -2. - k), v(-4. + k, 24. + k, 2. + k), 40., 32., aw, 12., 4., 0, false, 2);
    bx(&mut f, v(4., 12., -2.), v(4. + aw, 24., 2.), 32., 48., aw, 12., 4., 0, false, 3);
    bx(&mut f, v(4. - k, 12. - k, -2. - k), v(4. + aw + k, 24. + k, 2. + k), 48., 48., aw, 12., 4., 0, false, 3);
    bx(&mut f, v(-4., 0., -2.), v(0., 12., 2.), 0., 16., 4., 12., 4., 0, false, 4);
    bx(&mut f, v(-4. - k, 0. - k, -2. - k), v(0. + k, 12. + k, 2. + k), 0., 32., 4., 12., 4., 0, false, 4);
    bx(&mut f, v(0., 0., -2.), v(4., 12., 2.), 16., 48., 4., 12., 4., 0, false, 5);
    bx(&mut f, v(0. - k, 0. - k, -2. - k), v(4. + k, 12. + k, 2. + k), 0., 48., 4., 12., 4., 0, false, 5);
    if cape { bx(&mut f, v(-5., 8., -3.), v(5., 24., -2.), 0., 0., 10., 16., 1., 1, true, 6); }
    f
}
fn smooth(x: f32) -> f32 { let x = x.clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) }
// head yaw, head pitch, right arm, left arm
fn pose(t: f32) -> [f32; 4] {
    let c = t % 10.0;
    let w = if c < 4.0 { 0.0 } else if c < 5.2 { smooth((c - 4.0) / 1.2) } else if c < 8.0 { 1.0 } else if c < 9.2 { 1.0 - smooth((c - 8.0) / 1.2) } else { 0.0 };
    let br = (t * 1.5).sin();
    [w * -0.6 + (t * 0.45).sin() * 0.07 * (1.0 - w), w * 0.4 + (t * 0.8).sin() * 0.03,
     w * -1.2 + br * 0.04 * (1.0 - w) + (t * 2.6).sin() * 0.05 * w, -br * 0.04 + (t * 0.6).sin() * 0.02]
}
fn draw_char(painter: &egui::Painter, rect: egui::Rect, skin: egui::TextureId, cape: Option<egui::TextureId>, slim: bool, yaw: f32, pitch: f32, tm: Option<f32>) {
    let faces = model(slim, cape.is_some());
    let aw = if slim { 3.0 } else { 4.0 };
    let ps = tm.map(pose).unwrap_or([0.0; 4]);
    let s = rect.height() / 36.0;
    let c = rect.center();
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let rotv = |x: f32, y: f32, z: f32| -> (f32, f32, f32) {
        let x1 = x * cy + z * sy; let z1 = -x * sy + z * cy;
        let y1 = y * cp - z1 * sp; let z2 = y * sp + z1 * cp;
        (x1, y1, z2)
    };
    let xf = |part: u8, p: V3| -> V3 {
        let (px, py, pz, rx, ry) = match part {
            0 => (0.0, 24.0, 0.0, ps[1], ps[0]),
            2 => (-4.0 - aw / 2.0, 22.0, 0.0, ps[2], 0.0),
            3 => (4.0 + aw / 2.0, 22.0, 0.0, ps[3], 0.0),
            _ => return p,
        };
        let (x, y, z) = (p.x - px, p.y - py, p.z - pz);
        let (sx, cx) = rx.sin_cos();
        let y1 = y * cx - z * sx; let z1 = y * sx + z * cx;
        let (sy2, cy2) = ry.sin_cos();
        V3 { x: x * cy2 + z1 * sy2 + px, y: y1 + py, z: -x * sy2 + z1 * cy2 + pz }
    };
    let mut list: Vec<(f32, usize, [V3; 4])> = vec![];
    for (i, f) in faces.iter().enumerate() {
        let tc = [xf(f.part, f.c[0]), xf(f.part, f.c[1]), xf(f.part, f.c[2]), xf(f.part, f.c[3])];
        let e1 = (tc[1].x - tc[0].x, tc[1].y - tc[0].y, tc[1].z - tc[0].z);
        let e2 = (tc[3].x - tc[0].x, tc[3].y - tc[0].y, tc[3].z - tc[0].z);
        let n = (e2.1 * e1.2 - e2.2 * e1.1, e2.2 * e1.0 - e2.0 * e1.2, e2.0 * e1.1 - e2.1 * e1.0);
        let (_, _, nz) = rotv(n.0, n.1, n.2);
        if nz <= 0.0001 { continue; }
        let mut zs = 0.0;
        for k in 0..4 { zs += rotv(tc[k].x, tc[k].y - 16.0, tc[k].z).2; }
        list.push((zs / 4.0, i, tc));
    }
    list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, i, tc) in list {
        let f = &faces[i];
        let (tid, tw, th) = if f.tex == 1 { match cape { Some(t) => (t, 64.0, 32.0), None => continue } } else { (skin, 64.0, 64.0) };
        let (u0, v0, u1, v1) = (f.uv[0] / tw, f.uv[1] / th, (f.uv[0] + f.uv[2]) / tw, (f.uv[1] + f.uv[3]) / th);
        let uvs = [egui::pos2(u0, v0), egui::pos2(u1, v0), egui::pos2(u1, v1), egui::pos2(u0, v1)];
        let mut m = egui::Mesh::with_texture(tid);
        for k in 0..4 {
            let (x, y, _) = rotv(tc[k].x, tc[k].y - 16.0, tc[k].z);
            m.vertices.push(egui::epaint::Vertex { pos: egui::pos2(c.x + x * s, c.y - y * s), uv: uvs[k], color: Color32::WHITE });
        }
        m.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(m));
    }
}

// ---------- mob icons ----------
const MOBS: [[&str; 8]; 8] = [
    ["gdggggdg", "gggggggg", "gkkggkkg", "gkkggkkg", "gggkkggg", "ggkkkkgg", "ggkkkkgg", "ggkggkgg"],
    ["tbbbbbbt", "bbbbbbbb", "bkbbbbkb", "bbbbbbbb", "bcbbbbcb", "bbbbbbbb", "bbkkkkbb", ".bbbbbb."],
    ["pppppppp", "pppppppp", "pwkppkwp", "pppppppp", "ppsddspp", "ppsddspp", "pppppppp", "pppppppp"],
    ["hhhhhhhh", "hzzzzzzh", "zzzzzzzz", "zkkzzkkz", "zzzzzzzz", "zzzmmzzz", "zzmzzmzz", "zzzzzzzz"],
    ["nlnnnnln", "nnnnnnnn", "nnnnnnnn", "wppnnppw", "nnnnnnnn", "nnnnnnnn", "nlnnnnln", "nnnnnnnn"],
    ["aaaaaaaa", "aaaaaaaa", "akkaakka", "akkaakka", "aaaddaaa", "aadkkdaa", "aaaaaaaa", "aadadada"],
    [".ssssss.", "ssssssss", "skkssskk", "skkssskk", "ssssssss", "ssdddsss", "ssssssss", ".ssssss."],
    ["oyyyyyyo", "yyyyyyyy", "ykkyykky", "yyyyyyyy", "yyokkoyy", "yyyyyyyy", "oyyyyyyo", "yoyyyyoy"],
];
const GRADS: [([u8; 3], [u8; 3]); 8] = [
    ([59, 130, 246], [124, 58, 237]), ([20, 184, 166], [30, 64, 175]), ([251, 146, 60], [220, 38, 38]), ([244, 114, 182], [147, 51, 234]),
    ([100, 116, 139], [15, 23, 42]), ([250, 204, 21], [234, 88, 12]), ([56, 189, 248], [37, 99, 235]), ([167, 139, 250], [30, 27, 75]),
];
fn mob_px(m: u8, ch: u8) -> Option<Color32> {
    let c = |r: u8, g: u8, b: u8| Some(Color32::from_rgb(r, g, b));
    match (m, ch) {
        (0, b'g') => c(78, 170, 60), (0, b'd') => c(52, 120, 42), (0, b'k') => c(14, 22, 14),
        (1, b'b') => c(14, 38, 46), (1, b't') => c(34, 118, 128), (1, b'c') => c(80, 220, 230), (1, b'k') => c(6, 12, 16),
        (2, b'p') => c(240, 160, 160), (2, b'd') => c(214, 120, 126), (2, b's') => c(232, 128, 132), (2, b'w') => c(255, 255, 255), (2, b'k') => c(40, 30, 40),
        (3, b'z') => c(96, 146, 84), (3, b'h') => c(62, 94, 56), (3, b'm') => c(52, 78, 48), (3, b'k') => c(22, 28, 64),
        (4, b'n') => c(22, 22, 28), (4, b'l') => c(40, 40, 50), (4, b'p') => c(214, 110, 255), (4, b'w') => c(240, 200, 255),
        (5, b'a') => c(206, 206, 206), (5, b'd') => c(150, 150, 150), (5, b'k') => c(30, 30, 30),
        (6, b's') => c(110, 200, 120), (6, b'd') => c(70, 150, 86), (6, b'k') => c(20, 50, 30),
        (7, b'y') => c(250, 204, 64), (7, b'o') => c(232, 140, 32), (7, b'k') => c(30, 20, 10),
        _ => None,
    }
}
fn draw_mob(p: &egui::Painter, rc: egui::Rect, mob: u8, bg: u8) {
    let (a, b) = GRADS[(bg % 8) as usize];
    let (ca, cb) = (c3(a[0], a[1], a[2]), c3(b[0], b[1], b[2]));
    p.rect_filled(rc.translate(egui::vec2(0.0, 5.0)), 4.0, Color32::from_black_alpha(35));
    p.rect_filled(rc.translate(egui::vec2(0.0, 2.5)), 3.0, Color32::from_black_alpha(80));
    let cm = mix(ca, cb, 0.5);
    let mut m = egui::Mesh::default();
    for (pos, col) in [(rc.left_top(), ca), (rc.right_top(), cm), (rc.right_bottom(), cb), (rc.left_bottom(), cm)] {
        m.vertices.push(egui::epaint::Vertex { pos, uv: egui::epaint::WHITE_UV, color: col });
    }
    m.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    p.add(egui::Shape::mesh(m));
    p.rect_stroke(rc, 0.0, egui::Stroke::new(1.0, Color32::from_black_alpha(100)));
    let cell = (rc.width() * 0.72 / 8.0).max(1.0);
    let org = rc.center() - egui::vec2(cell * 4.0, cell * 4.0);
    for (y, row) in MOBS[(mob % 8) as usize].iter().enumerate() {
        for (x, ch) in row.bytes().enumerate() {
            if let Some(col) = mob_px(mob % 8, ch) {
                p.rect_filled(egui::Rect::from_min_size(org + egui::vec2(x as f32 * cell, y as f32 * cell), egui::vec2(cell + 0.5, cell + 0.5)), 0.0, col);
            }
        }
    }
}

// ---------- widgets ----------
fn card(ui: &mut egui::Ui, p: &Pal, f: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none().fill(p.card).rounding(8.0).inner_margin(egui::Margin::same(16.0))
        .stroke(egui::Stroke::new(1.0, p.stroke)).show(ui, |ui| { ui.set_width(ui.available_width()); f(ui) });
}
fn fmt(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1e6) } else if n >= 1000 { format!("{:.1}K", n as f64 / 1e3) } else { n.to_string() }
}
fn obtn(t: &str, c: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(t.to_string()).color(c).strong()).fill(Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, c)).rounding(6.0)
}
fn fbtn(t: &str, c: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(t.to_string()).color(Color32::WHITE).strong()).fill(c).rounding(6.0)
}
fn h1(ui: &mut egui::Ui, t: &str) { ui.label(RichText::new(t).size(24.0).family(egui::FontFamily::Name("bold".into()))); }
fn pill(ui: &mut egui::Ui, p: &Pal, text: &str, on: bool) -> egui::Response {
    let col = if on { Color32::WHITE } else { ui.visuals().text_color() };
    ui.add(egui::Button::new(RichText::new(text.to_string()).color(col).strong()).fill(if on { ACCENT } else { p.hover })
        .rounding(6.0).min_size(egui::vec2(0.0, 32.0)))
}
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(44.0, 24.0), egui::Sense::click());
    if resp.clicked() { *on = !*on; resp.mark_changed(); }
    let off = if ui.visuals().dark_mode { c3(80, 85, 96) } else { c3(190, 194, 202) };
    ui.painter().rect_filled(rect, 12.0, if *on { ACCENT } else { off });
    let x = if *on { rect.right() - 12.0 } else { rect.left() + 12.0 };
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 9.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn setting_row(ui: &mut egui::Ui, title: &str, desc: &str, on: &mut bool) -> bool {
    let mut ch = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - 80.0).max(120.0));
            ui.label(RichText::new(title).strong().size(16.0));
            if !desc.is_empty() { ui.label(RichText::new(desc).weak()); }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if toggle(ui, on).changed() { ch = true; } });
    });
    ui.add_space(6.0);
    ch
}
fn divider(ui: &mut egui::Ui, p: &Pal) {
    let (rc, _) = ui.allocate_exact_size(egui::vec2(30.0, 2.0), egui::Sense::hover());
    ui.painter().rect_filled(rc, 1.0, p.stroke);
}
fn draw_glyph(p: &egui::Painter, c: egui::Pos2, kind: &str, col: Color32) {
    let st = egui::Stroke::new(2.0, col);
    let pt = |x: f32, y: f32| egui::pos2(c.x + x, c.y + y);
    match kind {
        "home" => {
            p.add(egui::Shape::line(vec![pt(-10.0, -1.0), pt(0.0, -10.0), pt(10.0, -1.0)], st));
            p.add(egui::Shape::line(vec![pt(-7.0, -3.0), pt(-7.0, 9.0), pt(7.0, 9.0), pt(7.0, -3.0)], st));
            p.add(egui::Shape::line(vec![pt(-2.5, 9.0), pt(-2.5, 3.0), pt(2.5, 3.0), pt(2.5, 9.0)], st));
        }
        "skins" => {
            p.rect_stroke(egui::Rect::from_min_size(pt(-5.0, -10.0), egui::vec2(10.0, 9.0)), 2.0, st);
            p.add(egui::Shape::line(vec![pt(-9.0, 10.0), pt(-9.0, 3.0), pt(9.0, 3.0), pt(9.0, 10.0)], st));
        }
        "discover" => {
            p.circle_stroke(c, 10.0, st);
            p.add(egui::Shape::convex_polygon(vec![pt(5.0, -5.0), pt(1.5, 1.5), pt(-5.0, 5.0), pt(-1.5, -1.5)], col, egui::Stroke::NONE));
        }
        "plus" => { p.line_segment([pt(-9.0, 0.0), pt(9.0, 0.0)], st); p.line_segment([pt(0.0, -9.0), pt(0.0, 9.0)], st); }
        "settings" => {
            p.circle_stroke(c, 6.0, st);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4; let (s, co) = a.sin_cos();
                p.line_segment([pt(co * 8.5, s * 8.5), pt(co * 11.5, s * 11.5)], st);
            }
        }
        "logout" => {
            p.add(egui::Shape::line(vec![pt(1.0, -9.0), pt(-8.0, -9.0), pt(-8.0, 9.0), pt(1.0, 9.0)], st));
            p.line_segment([pt(-3.0, 0.0), pt(9.0, 0.0)], st);
            p.add(egui::Shape::line(vec![pt(5.0, -4.0), pt(9.0, 0.0), pt(5.0, 4.0)], st));
        }
        "shots" => {
            p.rect_stroke(egui::Rect::from_min_size(pt(-10.0, -6.0), egui::vec2(20.0, 15.0)), 3.0, st);
            p.circle_stroke(pt(0.0, 1.5), 4.0, st);
        }
        "panel" => {
            p.rect_stroke(egui::Rect::from_min_size(pt(-10.0, -8.0), egui::vec2(20.0, 16.0)), 3.0, st);
            p.line_segment([pt(3.0, -8.0), pt(3.0, 8.0)], st);
        }
        _ => {}
    }
}
fn side_icon(ui: &mut egui::Ui, kind: &str, sel: bool, p: &Pal) -> egui::Response {
    let (rc, r) = ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::click());
    let th = ui.ctx().animate_bool_with_time(r.id.with("h"), r.hovered(), 0.15);
    let ts = ui.ctx().animate_bool_with_time(r.id.with("s"), sel, 0.22);
    let bg = mix(mix(p.side, p.hover, th), mix(ACCENT, p.side, 0.78), ts);
    if th > 0.01 || ts > 0.01 { ui.painter().rect_filled(rc, 8.0, bg); }
    if ts > 0.01 {
        let h = 26.0 * ts;
        ui.painter().rect_filled(egui::Rect::from_center_size(egui::pos2(rc.left() - 9.0, rc.center().y), egui::vec2(3.0, h)), 1.5, ACCENT);
    }
    let base = mix(ui.visuals().text_color().linear_multiply(0.7), ui.visuals().strong_text_color(), th);
    draw_glyph(ui.painter(), rc.center() + egui::vec2(0.0, -1.5 * th), kind, mix(base, ACCENT, ts));
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn inst_icon(ui: &mut egui::Ui, i: &Inst, sel: bool, p: &Pal) -> egui::Response {
    let (rc, r) = ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::click());
    let th = ui.ctx().animate_bool_with_time(r.id.with("h"), r.hovered(), 0.15);
    let ts = ui.ctx().animate_bool_with_time(r.id.with("s"), sel, 0.22);
    let bg = mix(mix(p.side, p.hover, th), mix(ACCENT, p.side, 0.78), ts);
    if th > 0.01 || ts > 0.01 { ui.painter().rect_filled(rc, 8.0, bg); }
    if ts > 0.01 {
        ui.painter().rect_filled(egui::Rect::from_center_size(egui::pos2(rc.left() - 9.0, rc.center().y), egui::vec2(3.0, 26.0 * ts)), 1.5, ACCENT);
    }
    draw_mob(ui.painter(), egui::Rect::from_center_size(rc.center() + egui::vec2(0.0, -1.0), egui::vec2(30.0, 30.0)), i.icon, i.bg);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn inst_badge(ui: &mut egui::Ui, _p: &Pal, i: &Inst, sz: f32) {
    let (rc, _) = ui.allocate_exact_size(egui::vec2(sz + 4.0, sz + 6.0), egui::Sense::hover());
    draw_mob(ui.painter(), egui::Rect::from_min_size(rc.min, egui::vec2(sz, sz)), i.icon, i.bg);
}
fn inst_sub(i: &Inst, playtime: bool) -> String {
    let k = if i.kind == 1 { "Fabric + Warden" } else { "Vanilla" };
    if playtime { format!("{} \u{2022} {} \u{2022} {}", k, i.ver, fmt_time(i.played)) } else { format!("{} \u{2022} {}", k, i.ver) }
}
fn char_card(ui: &mut egui::Ui, p: &Pal, who: Option<(egui::TextureId, bool)>, cape: Option<egui::TextureId>, sel: bool) -> egui::Response {
    let (rc, r) = ui.allocate_exact_size(egui::vec2(176.0, 236.0), egui::Sense::click());
    let pt = ui.painter();
    let ht = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.15);
    pt.rect_filled(rc, 8.0, mix(p.card, p.hover, ht));
    pt.rect_stroke(rc, 8.0, egui::Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { ACCENT } else { p.stroke }));
    if let Some((t, sl)) = who { draw_char(&ui.painter_at(rc.shrink(6.0)), rc.shrink2(egui::vec2(18.0, 14.0)), t, cape, sl, 0.5, 0.12, None); }
    if sel {
        let c = rc.right_top() + egui::vec2(-20.0, 20.0);
        pt.circle_filled(c, 11.0, ACCENT);
        pt.add(egui::Shape::line(vec![c + egui::vec2(-4.5, 0.0), c + egui::vec2(-1.0, 3.5), c + egui::vec2(5.0, -3.5)], egui::Stroke::new(2.0, Color32::WHITE)));
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn add_card(ui: &mut egui::Ui, p: &Pal, title: &str, sub: &str) -> egui::Response {
    let (rc, r) = ui.allocate_exact_size(egui::vec2(176.0, 236.0), egui::Sense::click());
    let pt = ui.painter();
    let ht = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.15);
    pt.rect_filled(rc, 8.0, mix(p.card, p.hover, ht));
    pt.rect_stroke(rc, 8.0, egui::Stroke::new(1.5, p.stroke));
    let tc = ui.visuals().text_color();
    draw_glyph(pt, rc.center() + egui::vec2(0.0, -26.0), "plus", tc);
    pt.text(rc.center() + egui::vec2(0.0, 6.0), egui::Align2::CENTER_CENTER, title, egui::FontId::proportional(16.0), tc);
    pt.text(rc.center() + egui::vec2(0.0, 28.0), egui::Align2::CENTER_CENTER, sub, egui::FontId::proportional(13.0), tc.linear_multiply(0.6));
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn ms_button(ui: &mut egui::Ui, p: &Pal) -> egui::Response {
    let g = c3(34, 197, 94);
    let (rc, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
    let ht = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.12);
    ui.painter().rect_filled(rc, 6.0, mix(p.card, g, 0.12 + 0.07 * ht));
    ui.painter().rect_stroke(rc, 6.0, egui::Stroke::new(1.2, g));
    let (lx, ly) = (rc.center().x - 62.0, rc.center().y - 7.0);
    for (i, col) in [c3(242, 80, 34), c3(127, 186, 0), c3(0, 164, 239), c3(255, 185, 0)].iter().enumerate() {
        let (cx, cy) = ((i % 2) as f32, (i / 2) as f32);
        ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(lx + cx * 7.5, ly + cy * 7.5), egui::vec2(6.5, 6.5)), 0.0, *col);
    }
    ui.painter().text(egui::pos2(lx + 22.0, rc.center().y), egui::Align2::LEFT_CENTER, tr("Microsoft login"), egui::FontId::proportional(14.0), g);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn pick_file(sh: &Arc<Mutex<Shared>>, kind: u8) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        if let Some(p) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).pick_file() { sh.lock().unwrap().picked.push((kind, p)); }
    });
}
fn open_path(p: &Path) { let _ = Command::new("explorer").arg(p).spawn(); }
fn list_dir(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok())
        .map(|e| (e.file_name().to_string_lossy().to_string(), e.path())).collect()).unwrap_or_default();
    v.sort();
    v
}

// ---------- extra backend ----------
fn spawn_search2(sh: &Arc<Mutex<Shared>>, q: String, ptype: usize, page: usize) {
    let sh = sh.clone();
    let off = (page * 20).to_string();
    std::thread::spawn(move || {
        let pt = ["modpack", "mod", "resourcepack", "shader"][ptype.min(3)];
        let facets = if ptype == 1 { r#"[["project_type:mod"],["categories:fabric"]]"#.to_string() } else { format!(r#"[["project_type:{pt}"]]"#) };
        let res = (|| -> R<Value> {
            Ok(client().get("https://api.modrinth.com/v2/search")
                .query(&[("query", q.as_str()), ("facets", facets.as_str()), ("limit", "20"), ("offset", off.as_str())])
                .send()?.error_for_status()?.json()?)
        })();
        match res {
            Ok(r) => {
                sh.lock().unwrap().total = r["total_hits"].as_u64().unwrap_or(0);
                let arr = r["hits"].as_array().cloned().unwrap_or_default();
                let s = |h: &Value, k: &str| h[k].as_str().unwrap_or("").to_string();
                let hits: Vec<Hit> = arr.iter().map(|h| Hit { slug: s(h, "slug"), title: s(h, "title"), author: s(h, "author"),
                    desc: s(h, "description"), dl: h["downloads"].as_u64().unwrap_or(0), icon: None }).collect();
                sh.lock().unwrap().results = hits.clone();
                for (i, h) in arr.iter().enumerate() {
                    let u = s(h, "icon_url");
                    if u.is_empty() { continue; }
                    if let Ok(ci) = fetch_icon(&u) {
                        let mut g = sh.lock().unwrap();
                        if let Some(x) = g.results.get_mut(i) { if x.slug == hits[i].slug { x.icon = Some(ci); } }
                    }
                }
            }
            Err(e) => set(&sh, &format!("Search error: {e}")),
        }
    });
}
fn spawn_meta(sh: &Arc<Mutex<Shared>>, slug: &str) {
    let sh = sh.clone(); let slug = slug.to_string();
    std::thread::spawn(move || {
        let m = match json(&format!("https://api.modrinth.com/v2/project/{slug}")) {
            Ok(j) => {
                let v = |k: &str| j[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<String>>()).unwrap_or_default();
                (slug.clone(), v("game_versions"), v("loaders"))
            }
            Err(_) => (slug.clone(), vec![], vec![]),
        };
        sh.lock().unwrap().meta = Some(m);
    });
}
fn latest_version(slug: &str, mc: &str, loader: Option<&str>) -> R<Value> {
    let mut q: Vec<(&str, String)> = vec![("game_versions", format!("[\"{mc}\"]"))];
    if let Some(l) = loader { q.push(("loaders", format!("[\"{l}\"]"))); }
    let vs: Value = client().get(format!("https://api.modrinth.com/v2/project/{slug}/version")).query(&q).send()?.error_for_status()?.json()?;
    let v = vs.get(0).cloned().ok_or("no compatible version")?;
    Ok(v)
}
fn primary_file(v: &Value) -> R<(String, String)> {
    let a = v["files"].as_array().ok_or("no file")?;
    let f = a.iter().find(|f| f["primary"] == true).or(a.first()).ok_or("no file")?;
    Ok((f["url"].as_str().ok_or("no url")?.to_string(), f["filename"].as_str().ok_or("no name")?.to_string()))
}
fn safe_rel(p: &str) -> Option<PathBuf> {
    let pb = PathBuf::from(p);
    if pb.is_absolute() || pb.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_) | std::path::Component::RootDir)) { return None; }
    Some(pb)
}
fn install_mrpack(bytes: &[u8], dir: &Path) -> R<()> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let idx: Value = { let mut f = z.by_name("modrinth.index.json")?; let mut s = String::new(); f.read_to_string(&mut s)?; serde_json::from_str(&s)? };
    let files: Vec<(PathBuf, String)> = idx["files"].as_array().into_iter().flatten().filter(|f| f["env"]["client"] != "unsupported")
        .filter_map(|f| Some((safe_rel(f["path"].as_str()?)?, f["downloads"][0].as_str()?.to_string()))).collect();
    std::thread::scope(|s| {
        for ch in files.chunks(files.len() / 6 + 1) {
            s.spawn(move || for (p, u) in ch { if dl(u, &dir.join(p)).is_err() { let _ = dl(u, &dir.join(p)); } });
        }
    });
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let name = f.name().to_string();
        let rel = name.strip_prefix("overrides/").or_else(|| name.strip_prefix("client-overrides/"));
        let Some(rel) = rel else { continue };
        if f.is_dir() || rel.is_empty() { continue; }
        let Some(rp) = safe_rel(rel) else { continue };
        let out = dir.join(rp);
        if let Some(d) = out.parent() { fs::create_dir_all(d)?; }
        std::io::copy(&mut f, &mut fs::File::create(out)?)?;
    }
    Ok(())
}
fn install_any(ptype: usize, slug: &str, mc: &str, dir: &Path) -> R<()> {
    match ptype {
        1 => install_mod(slug, mc, &dir.join("mods"), &mut HashSet::new()),
        2 | 3 => {
            let v = latest_version(slug, mc, None)?;
            let (u, n) = primary_file(&v)?;
            dl(&u, &dir.join(if ptype == 2 { "resourcepacks" } else { "shaderpacks" }).join(n))
        }
        _ => {
            let v = latest_version(slug, mc, Some("fabric"))?;
            let (u, _) = primary_file(&v)?;
            install_mrpack(&get(&u)?, dir)
        }
    }
}
fn spawn_install_any(sh: &Arc<Mutex<Shared>>, ptype: usize, slug: String, mc: String, dir: PathBuf) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        set(&sh, &format!("Installing {slug}..."));
        let r = install_any(ptype, &slug, &mc, &dir);
        set(&sh, &match r { Ok(_) => format!("Installed {slug}"), Err(e) => format!("Error: {e}") });
    });
}
fn sync_options(insts: &[Inst], target: usize) {
    let src = insts.iter().enumerate().filter(|(i, x)| *i != target && x.last > 0 && inst_path(x).join("options.txt").exists())
        .max_by_key(|(_, x)| x.last).map(|(_, x)| inst_path(x));
    if let Some(s) = src {
        let d = inst_path(&insts[target]);
        let _ = fs::create_dir_all(&d);
        for f in ["options.txt", "servers.dat"] { let _ = fs::copy(s.join(f), d.join(f)); }
    }
}

fn cloud_key(name: &str, pw: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(format!("pw-warden:{}:{}", name.to_lowercase(), pw).as_bytes()))
}
fn cloud_push(sh: &Arc<Mutex<Shared>>, url: String, name: String, pw: String, insts: Vec<Inst>) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        let opts = insts.iter().max_by_key(|i| i.last).map(|i| inst_path(i).join("options.txt")).and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let list: Vec<Value> = insts.iter().map(|i| serde_json::json!({"name": i.name, "ver": i.ver, "kind": i.kind, "ram": i.ram})).collect();
        let body = serde_json::json!({"instances": list, "options": opts});
        let r = (|| -> R<()> {
            let resp = client().put(format!("{}/sync/{}", url.trim_end_matches('/'), name)).header("Authorization", cloud_key(&name, &pw)).body(body.to_string()).send()?;
            if resp.status().is_success() { Ok(()) } else { Err(format!("server said: {}", resp.text().unwrap_or_default()).into()) }
        })();
        set(&sh, &match r { Ok(_) => "Cloud sync: uploaded".to_string(), Err(e) => format!("Cloud sync error: {e}") });
    });
}
fn cloud_pull(sh: &Arc<Mutex<Shared>>, url: String, name: String, pw: String) {
    let sh = sh.clone();
    std::thread::spawn(move || {
        let r = (|| -> R<Value> {
            let resp = client().get(format!("{}/sync/{}", url.trim_end_matches('/'), name)).header("Authorization", cloud_key(&name, &pw)).send()?;
            if !resp.status().is_success() { return Err(format!("server said: {}", resp.text().unwrap_or_default()).into()); }
            Ok(serde_json::from_str(&resp.text()?)?)
        })();
        match r { Ok(v) => sh.lock().unwrap().cloud_in = Some(v), Err(e) => set(&sh, &format!("Cloud sync error: {e}")) }
    });
}

// ---------- app ----------
#[derive(Clone, Copy, PartialEq)]
enum Page { Home, Skins, Discover, Settings, Create, Shots, Inst(usize) }

struct App {
    sh: Arc<Mutex<Shared>>, cfg: Cfg, page: Page, pal: Pal, applied: i16, sys_light: bool, has_acct: bool,
    insts: Vec<Inst>, newname: String, newver: usize, newram: u32, newkind: u8, cid: String,
    uname: String, right_open: bool, itab: usize, stab: usize,
    dtype: usize, query: String, dpage: usize, want_search: bool, modal: Option<(String, String, usize)>, minst: String,
    yaw: f32,
    skins: Vec<SkinEntry>, capes: Vec<CapeEntry>, defs: Vec<SkinEntry>, guest: Option<SkinEntry>, acct_skin: Option<SkinEntry>, skins_dirty: bool,
    pg_key: Page, pg_t0: f64, modal_t0: f64, cloud_name: String, acct_menu: bool, chip_rect: egui::Rect, icon_edit: bool,
    pw: egui::TextureHandle, head: Option<egui::TextureHandle>, tex: HashMap<String, egui::TextureHandle>,
}

impl App {
    fn order(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.insts.len()).collect();
        v.sort_by(|a, b| self.insts[*b].last.cmp(&self.insts[*a].last).then(b.cmp(a)));
        v
    }
    fn cur_skin(&self) -> Option<&SkinEntry> {
        let s = self.cfg.skin.as_str();
        if let Some(n) = s.strip_prefix("d:") { if let Some(e) = self.defs.iter().find(|e| e.name == n) { return Some(e); } }
        else if !s.is_empty() { if let Some(e) = self.skins.iter().find(|e| e.name == s) { return Some(e); } }
        if self.has_acct { if let Some(e) = self.acct_skin.as_ref() { return Some(e); } }
        self.guest.as_ref().or(self.defs.first())
    }
    fn cur_cape(&self) -> Option<egui::TextureId> {
        if self.cfg.cape.is_empty() { return None; }
        self.capes.iter().find(|c| c.name == self.cfg.cape).map(|c| c.tex.id())
    }
    fn reload_skins(&mut self, ctx: &egui::Context) {
        self.skins.clear(); self.capes.clear();
        for (dir, is_cape) in [("skins", false), ("capes", true)] {
            let mut v: Vec<PathBuf> = fs::read_dir(root().join(dir)).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |x| x == "png")).collect()).unwrap_or_default();
            v.sort();
            for p in v {
                let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("x").to_string();
                if let Ok(im) = image::open(&p) {
                    let im = im.to_rgba8();
                    if is_cape {
                        let tex = ctx.load_texture(format!("cape_{name}"), cape_image(im), egui::TextureOptions::NEAREST);
                        self.capes.push(CapeEntry { name, tex });
                    } else {
                        let (n, sl) = norm_skin(im);
                        self.skins.push(mk_skin(ctx, &name, &n, sl));
                    }
                }
            }
        }
        self.skins_dirty = false;
    }
    fn import_file(&mut self, kind: u8, path: &Path) {
        if image::open(path).is_err() { set(&self.sh, "That file is not a valid PNG image."); return; }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("skin").replace(['/', '\\', ':', '|', '='], "_");
        let dir = root().join(if kind == 0 { "skins" } else { "capes" });
        let _ = fs::create_dir_all(&dir);
        let _ = fs::copy(path, dir.join(format!("{stem}.png")));
        if kind == 0 { self.cfg.skin = stem; } else { self.cfg.cape = stem; }
        write_cfg(&self.cfg);
        self.skins_dirty = true;
    }
    fn paint_head(&self, p: &egui::Painter, rc: egui::Rect, has_acct: bool) {
        let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        if has_acct { if let Some(h) = &self.head { p.image(h.id(), rc, full, Color32::WHITE); return; } }
        let first = if has_acct { self.acct_skin.as_ref().or(self.guest.as_ref()) } else { self.guest.as_ref() };
        let sk = first.or(self.defs.first());
        if let Some(s) = sk {
            let uv = |x: f32, y: f32| egui::Rect::from_min_max(egui::pos2(x / 64.0, y / 64.0), egui::pos2((x + 8.0) / 64.0, (y + 8.0) / 64.0));
            p.image(s.tex.id(), rc, uv(8.0, 8.0), Color32::WHITE);
            p.image(s.tex.id(), rc, uv(40.0, 8.0), Color32::WHITE);
        } else { p.rect_filled(rc, 0.0, c3(150, 105, 80)); }
        if !has_acct {
            let fs_ = rc.height() * 0.7;
            p.text(rc.center() + egui::vec2(1.5, 2.5), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(fs_), Color32::from_black_alpha(170));
            p.text(rc.center() + egui::vec2(0.0, 1.0), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(fs_), Color32::WHITE);
        }
    }
    fn start(&mut self, idx: usize, vers: &[(String, String)]) {
        let i = self.insts[idx].clone();
        if self.cfg.sync { sync_options(&self.insts, idx); }
        self.insts[idx].last = now_secs();
        save_insts(&self.insts);
        play(&self.sh, vers, &i, self.cfg.java.clone(), self.cid.clone());
    }

    fn cloud_ready(&self) -> Option<(String, String, String)> {
        let n = self.sh.lock().unwrap().acct.as_ref().map(|a| a.name.clone())?;
        if self.cfg.cloud_url.trim().is_empty() || self.cfg.cloud_pw.is_empty() { return None; }
        Some((self.cfg.cloud_url.trim().to_string(), n, self.cfg.cloud_pw.clone()))
    }
    fn cloud_up(&self) {
        if !self.cfg.cloud_on { return; }
        if let Some((u, n, p)) = self.cloud_ready() { cloud_push(&self.sh, u, n, p, self.insts.clone()); }
    }
    fn apply_cloud(&mut self, v: &Value) {
        let mut added = 0;
        for x in v["instances"].as_array().into_iter().flatten() {
            let name = x["name"].as_str().unwrap_or("").to_string();
            let ver = x["ver"].as_str().unwrap_or("").to_string();
            if name.is_empty() || ver.is_empty() || self.insts.iter().any(|i| i.name == name) { continue; }
            let i = Inst { name, ver, kind: x["kind"].as_u64().unwrap_or(0).min(1) as u8, ram: x["ram"].as_u64().unwrap_or(4096) as u32, last: 0, played: 0, icon: 0, bg: 1 };
            let d = inst_path(&i);
            if let Some(o) = v["options"].as_str().filter(|o| !o.is_empty()) { let _ = fs::create_dir_all(&d); let _ = fs::write(d.join("options.txt"), o); }
            if i.kind == 1 { spawn_install(&self.sh, PACK.iter().map(|s| s.to_string()).collect(), i.ver.clone(), d.join("mods")); }
            self.insts.push(i); added += 1;
        }
        save_insts(&self.insts);
        set(&self.sh, &format!("Cloud sync: downloaded {added} new instance(s)"));
    }

    fn account_menu(&mut self, ui: &mut egui::Ui, p: &Pal, acct: &Option<Acct>, login: &str) {
        let sh = self.sh.clone();
        ui.label(RichText::new(tr("Playing as")).weak().small());
        ui.horizontal(|ui| {
            let (rc, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
            self.paint_head(ui.painter(), rc, acct.is_some());
            ui.vertical(|ui| {
                ui.label(RichText::new(acct.as_ref().map_or(tr("Guest").to_string(), |a| a.name.clone())).strong().size(16.0));
                ui.label(RichText::new("Minecraft account").weak().small());
            });
        });
        ui.separator();
        if acct.is_none() {
            ui.add(egui::TextEdit::singleline(&mut self.uname).hint_text("Username").desired_width(f32::INFINITY));
            let n = self.uname.trim().to_string();
            let okn = n.len() >= 3 && n.len() <= 16 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            let w = ui.available_width();
            if ui.add_enabled(okn, fbtn(tr("Login"), ACCENT).min_size(egui::vec2(w, 32.0))).clicked() {
                let a = Acct { name: n.clone(), uuid: "00000000-0000-0000-0000-000000000000".into(), token: String::new(), refresh: "offline".into() };
                save_acct(&a); load_head(&sh, n); self.head = None; self.acct_skin = None;
                sh.lock().unwrap().acct = Some(a);
                self.acct_menu = false;
            }
            if ms_button(ui, p).clicked() { spawn_login(&sh, self.cid.clone()); }
        } else if ui.add(obtn(tr("Logout"), ACCENT)).clicked() {
            let _ = fs::remove_file(root().join("account.txt"));
            sh.lock().unwrap().acct = None; self.head = None; self.acct_skin = None;
            self.acct_menu = false;
        }
        if !login.is_empty() { ui.label(RichText::new(login).weak().small()); }
    }

    // ----- right panel -----
    fn right_panel(&mut self, ui: &mut egui::Ui, p: &Pal, acct: &Option<Acct>, login: &str) {
        let sh = self.sh.clone();
        ui.label(RichText::new(tr("Playing as")).strong().size(17.0));
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                let (rc, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                self.paint_head(ui.painter(), rc, acct.is_some());
                ui.vertical(|ui| {
                    ui.label(RichText::new(acct.as_ref().map_or(tr("Guest").to_string(), |a| a.name.clone())).strong().size(17.0));
                    ui.label(RichText::new("Minecraft account").weak().small());
                });
            });
        });
        if acct.is_none() {
            ui.add(egui::TextEdit::singleline(&mut self.uname).hint_text("Username").desired_width(f32::INFINITY));
            let n = self.uname.trim().to_string();
            let okn = n.len() >= 3 && n.len() <= 16 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if ui.add_enabled(okn, fbtn(tr("Login"), ACCENT)).clicked() {
                let a = Acct { name: n.clone(), uuid: "00000000-0000-0000-0000-000000000000".into(), token: String::new(), refresh: "offline".into() };
                save_acct(&a); load_head(&sh, n); self.head = None; self.acct_skin = None;
                sh.lock().unwrap().acct = Some(a);
            }
            if ui.add(obtn(tr("Microsoft login"), ACCENT)).clicked() { spawn_login(&sh, self.cid.clone()); }
        }
        if !login.is_empty() { ui.label(RichText::new(login).weak()); }
    }

    // ----- pages -----
    fn page_home(&mut self, ui: &mut egui::Ui, p: &Pal, acct: &Option<Acct>, busy: bool) -> Option<usize> {
        let mut go = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let sz = self.pw.size_vec2();
            ui.image((self.pw.id(), egui::vec2(46.0 * sz.x / sz.y, 46.0)));
            let name = acct.as_ref().map_or(tr("Guest").to_string(), |a| a.name.clone());
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Welcome back,")).size(28.0).strong());
                let (rc, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                self.paint_head(ui.painter(), rc, acct.is_some());
                ui.label(RichText::new(format!("{name}!")).size(28.0).strong());
            });
            // banner
            let w = ui.available_width();
            let (rc, _) = ui.allocate_exact_size(egui::vec2(w, 250.0), egui::Sense::hover());
            let tm = ui.input(|i| i.time) as f32;
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
            let pt = ui.painter_at(rc);
            let gy = rc.top() + rc.height() * 0.78;
            let n = 26;
            for k in 0..n {
                let t = k as f32 / (n - 1) as f32;
                let y0 = rc.top() + (gy - rc.top()) * k as f32 / n as f32;
                let y1 = rc.top() + (gy - rc.top()) * (k + 1) as f32 / n as f32 + 1.0;
                pt.rect_filled(egui::Rect::from_min_max(egui::pos2(rc.left(), y0), egui::pos2(rc.right(), y1)), 0.0, mix(c3(20, 28, 64), c3(104, 132, 200), t));
            }
            let bs = 22.0;
            for ix in 0..((w / bs) as i32 + 2) { for iy in 0..3 {
                let r = egui::Rect::from_min_size(egui::pos2(rc.left() + ix as f32 * bs, gy + iy as f32 * bs), egui::vec2(bs - 1.0, bs - 1.0));
                pt.rect_filled(r, 0.0, if iy == 0 { mix(ACCENT, c3(20, 28, 64), 0.25) } else { c3(38, 42, 62) });
            } }
            if let Some(sk) = self.cur_skin() {
                draw_char(&pt, egui::Rect::from_center_size(egui::pos2(rc.center().x, gy - 88.0), egui::vec2(120.0, 200.0)),
                    sk.tex.id(), self.cur_cape(), sk.slim, 0.4, 0.08, Some(tm));
            }
            let tag = egui::Rect::from_center_size(egui::pos2(rc.center().x, gy - 196.0), egui::vec2(name.len() as f32 * 9.0 + 24.0, 26.0));
            pt.rect_filled(tag, 4.0, Color32::from_black_alpha(150));
            pt.text(tag.center(), egui::Align2::CENTER_CENTER, &name, egui::FontId::proportional(15.0), Color32::WHITE);
            ui.add_space(14.0);
            ui.label(RichText::new(tr("Jump in")).size(22.0).strong());
            let ord = self.order();
            if ord.is_empty() {
                card(ui, p, |ui| {
                    ui.label(RichText::new("No instances yet").strong().size(17.0));
                    ui.label(RichText::new("Create your first instance to start playing.").weak());
                    if ui.add(fbtn(tr("Create instance"), ACCENT)).clicked() { self.page = Page::Create; }
                });
            }
            for idx in ord.into_iter().take(6) {
                let i = self.insts[idx].clone();
                let compact = self.cfg.compact;
                let pt_ = self.cfg.playtime;
                let mut body = |ui: &mut egui::Ui| {
                    ui.horizontal(|ui| {
                        inst_badge(ui, p, &i, if compact { 34.0 } else { 52.0 });
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&i.name).strong().size(17.0));
                            ui.label(RichText::new(inst_sub(&i, pt_)).weak());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add_enabled(!busy, fbtn(tr("Play"), ACCENT)).clicked() { go = Some(idx); }
                            if ui.add(obtn(tr("Open"), ACCENT)).clicked() { self.page = Page::Inst(idx); }
                        });
                    });
                };
                if compact { body(ui); } else { card(ui, p, body); }
                ui.add_space(4.0);
            }
        });
        go
    }

    fn page_skins(&mut self, ui: &mut egui::Ui, p: &Pal) {
        let sh = self.sh.clone();
        let name = if self.has_acct { self.insts.first().map(|_| String::new()).unwrap_or_default() } else { String::new() };
        let _ = name;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(270.0);
                h1(ui, tr("Skin selector"));
                let tag = if self.has_acct { sh.lock().unwrap().acct.as_ref().map(|a| a.name.clone()).unwrap_or_default() } else { tr("Guest").to_string() };
                if !self.cfg.hide_tag {
                    egui::Frame::none().fill(p.card).rounding(8.0).inner_margin(egui::Margin::symmetric(14.0, 6.0))
                        .show(ui, |ui| { ui.label(RichText::new(tag).monospace().strong()); });
                }
                let (resp, painter) = ui.allocate_painter(egui::vec2(270.0, 400.0), egui::Sense::drag());
                if resp.dragged() { self.yaw += resp.drag_delta().x * 0.015; }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
                if let Some(sk) = self.cur_skin() {
                    draw_char(&painter, resp.rect.shrink2(egui::vec2(30.0, 0.0)), sk.tex.id(), self.cur_cape(), sk.slim, self.yaw, 0.12, Some(ui.input(|i| i.time) as f32));
                }
                ui.label(RichText::new(tr("Drag to rotate")).weak());
                if ui.add(obtn("Open skins folder", ACCENT)).clicked() { let d = root().join("skins"); let _ = fs::create_dir_all(&d); open_path(&d); }
            });
            ui.add_space(20.0);
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let mut sel: Option<String> = None;
                    let mut rm: Option<usize> = None;
                    let mut cape_sel: Option<String> = None;
                    let mut cape_rm: Option<usize> = None;
                    ui.label(RichText::new(tr("Saved skins")).size(20.0).strong());
                    ui.horizontal_wrapped(|ui| {
                        if add_card(ui, p, tr("Add skin"), "Drag and drop").clicked() { pick_file(&sh, 0); }
                        for (i, e) in self.skins.iter().enumerate() {
                            let r = char_card(ui, p, Some((e.tex.id(), e.slim)), self.cur_cape(), self.cfg.skin == e.name);
                            if r.clicked() { sel = Some(e.name.clone()); }
                            r.context_menu(|ui| { if ui.button("Remove").clicked() { rm = Some(i); ui.close_menu(); } });
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Default skins")).size(20.0).strong());
                    ui.horizontal_wrapped(|ui| {
                        let own = self.cfg.skin.is_empty();
                        if let Some(g) = self.acct_skin.as_ref().filter(|_| self.has_acct).or(self.guest.as_ref()) {
                            if char_card(ui, p, Some((g.tex.id(), g.slim)), self.cur_cape(), own).clicked() { sel = Some(String::new()); }
                        }
                        for e in self.defs.iter() {
                            let key = format!("d:{}", e.name);
                            if char_card(ui, p, Some((e.tex.id(), e.slim)), self.cur_cape(), self.cfg.skin == key).clicked() { sel = Some(key); }
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Capes")).size(20.0).strong());
                    ui.horizontal_wrapped(|ui| {
                        if add_card(ui, p, tr("No cape"), "").clicked() { cape_sel = Some(String::new()); }
                        for (i, c) in self.capes.iter().enumerate() {
                            let (rc, r) = ui.allocate_exact_size(egui::vec2(120.0, 236.0), egui::Sense::click());
                            let on = self.cfg.cape == c.name;
                            ui.painter().rect_filled(rc, 8.0, if r.hovered() { p.hover } else { p.card });
                            ui.painter().rect_stroke(rc, 8.0, egui::Stroke::new(if on { 2.0 } else { 1.0 }, if on { ACCENT } else { p.stroke }));
                            let ir = egui::Rect::from_center_size(rc.center(), egui::vec2(80.0, 128.0));
                            ui.painter().image(c.tex.id(), ir, egui::Rect::from_min_max(egui::pos2(1.0 / 64.0, 1.0 / 32.0), egui::pos2(11.0 / 64.0, 17.0 / 32.0)), Color32::WHITE);
                            if r.clicked() { cape_sel = Some(c.name.clone()); }
                            r.context_menu(|ui| { if ui.button("Remove").clicked() { cape_rm = Some(i); ui.close_menu(); } });
                        }
                        if add_card(ui, p, tr("Add cape"), "PNG 64x32").clicked() { pick_file(&sh, 1); }
                    });
                    ui.label(RichText::new("Right-click a saved skin or cape to remove it. Skins and capes are previews inside the launcher; Ears mod parts are not rendered.").weak().small());
                    if let Some(n) = sel { self.cfg.skin = n; write_cfg(&self.cfg); }
                    if let Some(n) = cape_sel { self.cfg.cape = n; write_cfg(&self.cfg); }
                    if let Some(i) = rm {
                        let nm = self.skins[i].name.clone();
                        let _ = fs::remove_file(root().join("skins").join(format!("{nm}.png")));
                        if self.cfg.skin == nm { self.cfg.skin.clear(); write_cfg(&self.cfg); }
                        self.skins_dirty = true;
                    }
                    if let Some(i) = cape_rm {
                        let nm = self.capes[i].name.clone();
                        let _ = fs::remove_file(root().join("capes").join(format!("{nm}.png")));
                        if self.cfg.cape == nm { self.cfg.cape.clear(); write_cfg(&self.cfg); }
                        self.skins_dirty = true;
                    }
                });
            });
        });
    }

    fn page_discover(&mut self, ui: &mut egui::Ui, p: &Pal, hits: &[Hit], total: u64) {
        let sh = self.sh.clone();
        h1(ui, tr("Discover content"));
        ui.horizontal(|ui| {
            for (i, l) in [tr("Modpacks"), tr("Mods"), tr("Resource Packs"), tr("Shaders")].iter().enumerate() {
                if pill(ui, p, l, self.dtype == i).clicked() { self.dtype = i; self.dpage = 0; self.want_search = true; }
            }
        });
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search...").desired_width((ui.available_width() - 110.0).max(100.0)));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.add(fbtn(tr("Search"), ACCENT)).clicked() || enter { self.dpage = 0; self.want_search = true; }
        });
        if self.want_search { self.want_search = false; spawn_search2(&sh, self.query.clone(), self.dtype, self.dpage); }
        ui.horizontal(|ui| {
            if self.dpage > 0 && ui.add(obtn("Prev", ACCENT)).clicked() { self.dpage -= 1; self.want_search = true; }
            ui.label(RichText::new(format!("{} results \u{2022} page {}", fmt(total), self.dpage + 1)).weak());
            if (self.dpage + 1) as u64 * 20 < total && ui.add(obtn("Next", ACCENT)).clicked() { self.dpage += 1; self.want_search = true; }
        });
        let mut open: Option<(String, String)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for h in hits {
                card(ui, p, |ui| {
                    ui.horizontal(|ui| {
                        if let Some(t) = self.tex.get(&h.slug) { ui.image((t.id(), egui::vec2(52.0, 52.0))); } else { ui.add_space(52.0); }
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 120.0).max(140.0));
                            ui.label(RichText::new(&h.title).strong().size(17.0));
                            ui.label(RichText::new(format!("by {} \u{2022} {} downloads", h.author, fmt(h.dl))).weak().small());
                            ui.label(&h.desc);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(fbtn(tr("Install"), ACCENT)).clicked() { open = Some((h.slug.clone(), h.title.clone())); }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some((s, t)) = open {
            self.modal = Some((s.clone(), t, self.dtype)); self.minst.clear(); self.modal_t0 = ui.input(|i| i.time);
            sh.lock().unwrap().meta = None;
            spawn_meta(&sh, &s);
        }
    }

    fn install_modal(&mut self, ctx: &egui::Context, p: &Pal, meta: &Option<(String, Vec<String>, Vec<String>)>) {
        let Some((slug, title, ptype)) = self.modal.clone() else { return };
        egui::Area::new(egui::Id::new("dim")).order(egui::Order::Background).fixed_pos(egui::pos2(0.0, 0.0)).show(ctx, |ui| {
            let r = ctx.screen_rect();
            ui.allocate_rect(r, egui::Sense::click());
            let a = (((ctx.input(|i| i.time) - self.modal_t0) / 0.2).clamp(0.0, 1.0) * 150.0) as u8;
            if a < 150 { ctx.request_repaint(); }
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(a));
        });
        let sh = self.sh.clone();
        let mut close = false;
        let mut install: Option<usize> = None;
        egui::Window::new("install_project").title_bar(false).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).fixed_size([460.0, 500.0])
            .frame(egui::Frame::none().fill(p.card).rounding(16.0).stroke(egui::Stroke::new(1.0, p.stroke)).inner_margin(egui::Margin::same(20.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Install project")).size(22.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (rc, r) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
                        ui.painter().circle_filled(rc.center(), 16.0, if r.hovered() { p.stroke } else { p.hover });
                        let st = egui::Stroke::new(2.0, ui.visuals().text_color());
                        ui.painter().line_segment([rc.center() + egui::vec2(-5.0, -5.0), rc.center() + egui::vec2(5.0, 5.0)], st);
                        ui.painter().line_segment([rc.center() + egui::vec2(-5.0, 5.0), rc.center() + egui::vec2(5.0, -5.0)], st);
                        if r.clicked() { close = true; }
                    });
                });
                ui.label(RichText::new(&title).weak());
                ui.separator();
                ui.label(RichText::new("Instance type").strong());
                egui::Frame::none().fill(mix(ACCENT, p.card, 0.8)).stroke(egui::Stroke::new(1.0, ACCENT)).rounding(10.0)
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0)).show(ui, |ui| { ui.label(RichText::new("Existing instance").color(ACCENT).strong()); });
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut self.minst).hint_text("Search instance").desired_width(f32::INFINITY));
                let mut count = 0;
                match meta {
                    Some((s, gv, ld)) if *s == slug => {
                        let q = self.minst.to_lowercase();
                        egui::ScrollArea::vertical().max_height(270.0).auto_shrink([false, false]).show(ui, |ui| {
                            for (idx, i) in self.insts.iter().enumerate() {
                                let fab = i.kind == 1;
                                let ok = gv.contains(&i.ver) && match ptype { 0 | 1 => fab && ld.iter().any(|l| l == "fabric"), 2 => true, _ => fab };
                                if !ok || !i.name.to_lowercase().contains(&q) { continue; }
                                count += 1;
                                ui.horizontal(|ui| {
                                    inst_badge(ui, p, i, 38.0);
                                    ui.vertical(|ui| { ui.label(RichText::new(&i.name).strong()); ui.label(RichText::new(inst_sub(i, false)).weak().small()); });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.add(fbtn(tr("Install"), ACCENT)).clicked() { install = Some(idx); }
                                    });
                                });
                            }
                        });
                    }
                    _ => { ui.label(RichText::new("Loading...").weak()); }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} compatible instances", count)).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(obtn(tr("Cancel"), ui.visuals().text_color())).clicked() { close = true; }
                        });
                    });
                });
            });
        if let Some(idx) = install {
            let i = self.insts[idx].clone();
            spawn_install_any(&sh, ptype, slug, i.ver.clone(), inst_path(&i));
            close = true;
        }
        if close { self.modal = None; }
    }

    fn page_create(&mut self, ui: &mut egui::Ui, p: &Pal, vers: &[(String, String)]) {
        let sh = self.sh.clone();
        h1(ui, tr("Create instance"));
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                if pill(ui, p, "Vanilla", self.newkind == 0).clicked() { self.newkind = 0; }
                if pill(ui, p, "Fabric + Warden", self.newkind == 1).clicked() { self.newkind = 1; }
            });
            ui.label(RichText::new(if self.newkind == 1 { "Fabric with Sodium, Lithium, Iris and other performance mods, installed automatically." } else { "Plain Minecraft, no mods." }).weak());
            ui.add(egui::TextEdit::singleline(&mut self.newname).hint_text("Instance name").desired_width(f32::INFINITY));
            let cur = vers.get(self.newver).map(|v| v.0.clone()).unwrap_or_else(|| "...".into());
            egui::ComboBox::from_id_salt("newver").selected_text(cur).height(300.0).show_ui(ui, |ui| {
                for (k, v) in vers.iter().enumerate() { ui.selectable_value(&mut self.newver, k, &v.0); }
            });
            ui.add(egui::Slider::new(&mut self.newram, 1024..=16384).step_by(512.0).text("RAM (MB)"));
            ui.horizontal(|ui| {
                let ok = !self.newname.trim().is_empty() && !vers.is_empty();
                if ui.add_enabled(ok, fbtn(tr("Create"), ACCENT)).clicked() {
                    let mut name = self.newname.trim().to_string();
                    while self.insts.iter().any(|i| i.name == name) { name.push_str(" 2"); }
                    let i = Inst { name, ver: vers[self.newver.min(vers.len() - 1)].0.clone(), kind: self.newkind, ram: self.newram, last: now_secs(), played: 0, icon: self.newkind, bg: (self.insts.len() % 8) as u8 };
                    if i.kind == 1 { spawn_install(&sh, PACK.iter().map(|s| s.to_string()).collect(), i.ver.clone(), inst_path(&i).join("mods")); }
                    self.insts.push(i);
                    save_insts(&self.insts);
                    self.cloud_up();
                    self.newname.clear();
                    self.page = Page::Inst(self.insts.len() - 1);
                }
                if ui.add(obtn(tr("Cancel"), ACCENT)).clicked() { self.page = Page::Home; }
            });
        });
    }

    fn page_inst(&mut self, ui: &mut egui::Ui, p: &Pal, idx: usize, busy: bool) -> Option<usize> {
        if idx >= self.insts.len() { self.page = Page::Home; return None; }
        let i = self.insts[idx].clone();
        let dir = inst_path(&i);
        let sh = self.sh.clone();
        let mut go = None;
        let mut del = false;
        ui.horizontal(|ui| {
            inst_badge(ui, p, &i, 64.0);
            ui.vertical(|ui| { h1(ui, &i.name); ui.label(RichText::new(inst_sub(&i, self.cfg.playtime)).weak()); });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!busy, fbtn(tr("Play"), ACCENT).min_size(egui::vec2(110.0, 38.0))).clicked() { go = Some(idx); }
                if ui.add(obtn(tr("Delete"), ACCENT)).clicked() { del = true; }
                if ui.add(obtn("Folder", ACCENT)).clicked() { let _ = fs::create_dir_all(&dir); open_path(&dir); }
                if ui.add(obtn("Customize", ACCENT)).clicked() { self.icon_edit = !self.icon_edit; }
            });
        });
        if self.icon_edit {
            card(ui, p, |ui| {
                ui.label(RichText::new("Icon").strong());
                ui.horizontal_wrapped(|ui| {
                    for m in 0..8u8 {
                        let (rc, r) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
                        draw_mob(ui.painter(), rc, m, self.insts[idx].bg);
                        if m == self.insts[idx].icon { ui.painter().rect_stroke(rc.expand(3.0), 2.0, egui::Stroke::new(2.0, ACCENT)); }
                        if r.clicked() { self.insts[idx].icon = m; save_insts(&self.insts); }
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new("Background").strong());
                ui.horizontal_wrapped(|ui| {
                    for g in 0..8u8 {
                        let (rc, r) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
                        draw_mob(ui.painter(), rc, self.insts[idx].icon, g);
                        if g == self.insts[idx].bg { ui.painter().rect_stroke(rc.expand(3.0), 2.0, egui::Stroke::new(2.0, ACCENT)); }
                        if r.clicked() { self.insts[idx].bg = g; save_insts(&self.insts); }
                    }
                });
            });
        }
        let mut tabs: Vec<(&str, usize)> = vec![("Content", 0)];
        if self.cfg.worlds { tabs.push(("Worlds", 1)); }
        if self.cfg.files { tabs.push(("Files", 2)); }
        if self.cfg.shots { tabs.push(("Screenshots", 3)); }
        if !tabs.iter().any(|t| t.1 == self.itab) { self.itab = 0; }
        ui.horizontal(|ui| { for (l, k) in &tabs { if pill(ui, p, l, self.itab == *k).clicked() { self.itab = *k; } } });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match self.itab {
                0 => {
                    for (title, sub) in [("Mods", "mods"), ("Resource packs", "resourcepacks"), ("Shaders", "shaderpacks")] {
                        let items = list_dir(&dir.join(sub));
                        if items.is_empty() { continue; }
                        ui.label(RichText::new(title).strong().size(18.0));
                        for (n, path) in items {
                            card(ui, p, |ui| { ui.horizontal(|ui| {
                                ui.label(&n);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Remove").clicked() { let _ = fs::remove_file(&path); }
                                });
                            }); });
                        }
                    }
                    ui.label(RichText::new("Get more content in Discover content.").weak());
                }
                1 => { for (n, path) in list_dir(&dir.join("saves")) { if path.is_dir() { card(ui, p, |ui| { ui.label(n); }); } } }
                2 => {
                    if ui.add(obtn("Open folder", ACCENT)).clicked() { let _ = fs::create_dir_all(&dir); open_path(&dir); }
                    for (n, _) in list_dir(&dir) { ui.label(n); }
                }
                _ => {
                    for (n, path) in list_dir(&dir.join("screenshots")) {
                        if ui.add(obtn(&n, ACCENT)).clicked() { let _ = Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn(); }
                    }
                }
            }
        });
        if del {
            self.insts.remove(idx); save_insts(&self.insts); self.page = Page::Home;
            return None;
        }
        go
    }

    fn page_shots(&mut self, ui: &mut egui::Ui, p: &Pal) {
        h1(ui, "Screenshots");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut any = false;
            for i in &self.insts {
                for (n, path) in list_dir(&inst_path(i).join("screenshots")).into_iter().take(100) {
                    any = true;
                    card(ui, p, |ui| { ui.horizontal(|ui| {
                        ui.label(RichText::new(&i.name).strong());
                        ui.label(RichText::new(&n).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(obtn(tr("Open"), ACCENT)).clicked() { let _ = Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn(); }
                        });
                    }); });
                }
            }
            if !any { ui.label(RichText::new("No screenshots yet.").weak()); }
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui, p: &Pal) {
        let mut ch = false;
        h1(ui, tr("Settings"));
        ui.horizontal(|ui| {
            for (i, l) in [tr("Appearance"), tr("Features"), tr("Behavior"), tr("Language")].iter().enumerate() {
                if pill(ui, p, l, self.stab == i).clicked() { self.stab = i; }
            }
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match self.stab {
                0 => {
                    ui.label(RichText::new(tr("Color theme")).strong().size(18.0));
                    ui.label(RichText::new("Select your preferred color theme.").weak());
                    ui.horizontal_wrapped(|ui| {
                        for (i, nm) in ["Dark", "Light", "OLED", "Sync with system"].iter().enumerate() {
                            let (rc, r) = ui.allocate_exact_size(egui::vec2(210.0, 128.0), egui::Sense::click());
                            let pp = pal_for(match i { 0 => 0, 1 => 1, 2 => 2, _ => if self.sys_light { 1 } else { 0 } });
                            let sel = self.cfg.theme as usize == i;
                            let pt = ui.painter();
                            let top = egui::Rect::from_min_max(rc.min, egui::pos2(rc.right(), rc.bottom() - 38.0));
                            pt.rect_filled(top, egui::Rounding { nw: 12.0, ne: 12.0, sw: 0.0, se: 0.0 }, pp.bg);
                            let inner = egui::Rect::from_min_size(top.min + egui::vec2(26.0, 20.0), egui::vec2(158.0, 52.0));
                            pt.rect_filled(inner, 8.0, pp.card);
                            pt.rect_stroke(inner, 8.0, egui::Stroke::new(1.0, pp.stroke));
                            pt.rect_filled(egui::Rect::from_min_size(inner.min + egui::vec2(10.0, 10.0), egui::vec2(30.0, 30.0)), 6.0, pp.hover);
                            let bar = if pp.light { c3(60, 62, 70) } else { c3(200, 205, 215) };
                            pt.rect_filled(egui::Rect::from_min_size(inner.min + egui::vec2(50.0, 14.0), egui::vec2(90.0, 8.0)), 4.0, bar);
                            pt.rect_filled(egui::Rect::from_min_size(inner.min + egui::vec2(50.0, 30.0), egui::vec2(60.0, 7.0)), 3.5, mix(bar, pp.card, 0.55));
                            let strip = egui::Rect::from_min_max(egui::pos2(rc.left(), rc.bottom() - 38.0), rc.max);
                            pt.rect_filled(strip, egui::Rounding { nw: 0.0, ne: 0.0, sw: 12.0, se: 12.0 }, mix(p.card, p.hover, 0.6));
                            let rcen = egui::pos2(rc.left() + 22.0, rc.bottom() - 19.0);
                            pt.circle_stroke(rcen, 7.0, egui::Stroke::new(1.5, if sel { ACCENT } else { c3(130, 136, 148) }));
                            if sel { pt.circle_filled(rcen, 4.0, ACCENT); }
                            pt.text(egui::pos2(rc.left() + 40.0, rc.bottom() - 19.0), egui::Align2::LEFT_CENTER, tr(*nm), egui::FontId::proportional(15.0), ui.visuals().text_color());
                            pt.rect_stroke(rc, 12.0, egui::Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { ACCENT } else { p.stroke }));
                            if r.clicked() { self.cfg.theme = i as u8; ch = true; if i == 3 { self.sys_light = system_light(); } }
                        }
                    });
                }
                1 => {
                    ch |= setting_row(ui, tr("Sync features across instances"), "Copy your Minecraft settings (controls, video, sounds, servers) from the instance you played last to the one you launch.", &mut self.cfg.sync);
                    ui.separator();
                    ui.label(RichText::new("Cloud account sync").strong().size(20.0));
                    ui.label(RichText::new("Keep your instances and Minecraft settings on your account and download them on another computer. Needs a sync server (see the cloud folder).").weak());
                    ch |= setting_row(ui, "Cloud sync", "Upload after playing, download when you log in.", &mut self.cfg.cloud_on);
                    if ui.add(egui::TextEdit::singleline(&mut self.cfg.cloud_url).hint_text("Sync server URL (https://...)").desired_width(f32::INFINITY)).changed() { ch = true; }
                    if ui.add(egui::TextEdit::singleline(&mut self.cfg.cloud_pw).password(true).hint_text("Sync password (the first upload sets it)").desired_width(f32::INFINITY)).changed() { ch = true; }
                    ui.horizontal(|ui| {
                        if ui.add(fbtn("Upload now", ACCENT)).clicked() {
                            match self.cloud_ready() { Some((u, n, p)) => cloud_push(&self.sh, u, n, p, self.insts.clone()), None => set(&self.sh, "Log in and fill the server URL and password first.") }
                        }
                        if ui.add(obtn("Download", ACCENT)).clicked() {
                            match self.cloud_ready() { Some((u, n, p)) => cloud_pull(&self.sh, u, n, p), None => set(&self.sh, "Log in and fill the server URL and password first.") }
                        }
                    });
                    ui.separator();
                    ui.label(RichText::new(tr("Instances")).strong().size(20.0));
                    ch |= setting_row(ui, "Show Worlds tab in instances", "", &mut self.cfg.worlds);
                    ch |= setting_row(ui, "Show Files tab in instances", "", &mut self.cfg.files);
                    ch |= setting_row(ui, "Show Screenshots tab in instances", "", &mut self.cfg.shots);
                    ui.separator();
                    ui.label(RichText::new(tr("Sidebar")).strong().size(20.0));
                    ch |= setting_row(ui, "Show all screenshots in sidebar", "Show a button in the left sidebar to view screenshots from all your instances.", &mut self.cfg.side_shots);
                    ch |= setting_row(ui, "Show skin selector in sidebar", "Show a button in the left sidebar to open the skin selector.", &mut self.cfg.side_skins);
                    ui.label(RichText::new("Quick instances in sidebar").strong().size(16.0));
                    ui.horizontal(|ui| {
                        ui.label("0");
                        if ui.add(egui::Slider::new(&mut self.cfg.quick, 0..=20)).changed() { ch = true; }
                        ui.label("20");
                    });
                    ui.label(RichText::new("Choose the maximum number of recent instances shown in the left sidebar.").weak());
                }
                2 => {
                    ui.label(RichText::new(tr("Startup and navigation")).strong().size(20.0));
                    ch |= setting_row(ui, tr("Minimize app"), "Minimize the launcher when Minecraft starts.", &mut self.cfg.minimize);
                    ch |= setting_row(ui, "Refocus app when Minecraft closes", "Bring the launcher to the foreground when Minecraft exits.", &mut self.cfg.refocus);
                    ui.separator();
                    ui.label(RichText::new(tr("Home and content")).strong().size(20.0));
                    ch |= setting_row(ui, tr("Compact mode"), "Display instances in a compact row layout.", &mut self.cfg.compact);
                    ch |= setting_row(ui, tr("Show play time"), "Show how long you've played each instance.", &mut self.cfg.playtime);
                    ch |= setting_row(ui, tr("Hide nametag"), "Hide your username above the player preview on the Skins page.", &mut self.cfg.hide_tag);
                    ui.separator();
                    ui.label(RichText::new("Java").strong().size(20.0));
                    ui.label(RichText::new("Leave empty to let the launcher download the right Java automatically.").weak());
                    if ui.add(egui::TextEdit::singleline(&mut self.cfg.java).hint_text("Path to java.exe (optional)").desired_width(f32::INFINITY)).changed() { ch = true; }
                    if ui.add(obtn("Open launcher folder", ACCENT)).clicked() { let _ = fs::create_dir_all(root()); open_path(&root()); }
                }
                _ => {
                    ui.label(RichText::new("All European languages are translated; other languages show English text for now.").weak().small());
                    for (code, native, english, pct) in LANGS.iter() {
                        let sel = self.cfg.lang == *code;
                        let (rc, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::click());
                        let pt = ui.painter();
                        pt.rect_filled(rc, 10.0, if sel { mix(ACCENT, p.bg, 0.82) } else if r.hovered() { p.hover } else { p.card });
                        pt.rect_stroke(rc, 10.0, egui::Stroke::new(if sel { 1.5 } else { 1.0 }, if sel { ACCENT } else { p.stroke }));
                        let b = egui::Rect::from_min_size(rc.min + egui::vec2(12.0, 11.0), egui::vec2(34.0, 24.0));
                        pt.rect_filled(b, 5.0, p.hover);
                        pt.text(b.center(), egui::Align2::CENTER_CENTER, code.split('-').next().unwrap_or("").to_uppercase(), egui::FontId::proportional(12.0), ui.visuals().text_color());
                        let tc = ui.visuals().strong_text_color();
                        if native.is_empty() {
                            pt.text(egui::pos2(rc.left() + 58.0, rc.center().y), egui::Align2::LEFT_CENTER, *english, egui::FontId::proportional(16.0), tc);
                        } else {
                            let r1 = pt.text(egui::pos2(rc.left() + 58.0, rc.center().y), egui::Align2::LEFT_CENTER, *native, egui::FontId::proportional(16.0), tc);
                            pt.text(egui::pos2(r1.right() + 10.0, rc.center().y), egui::Align2::LEFT_CENTER, *english, egui::FontId::proportional(14.0), tc.linear_multiply(0.55));
                        }
                        pt.text(egui::pos2(rc.right() - 60.0, rc.center().y), egui::Align2::RIGHT_CENTER, format!("{}%", pct), egui::FontId::proportional(14.0), tc.linear_multiply(0.55));
                        let rcen = egui::pos2(rc.right() - 26.0, rc.center().y);
                        pt.circle_stroke(rcen, 8.0, egui::Stroke::new(1.5, if sel { ACCENT } else { c3(130, 136, 148) }));
                        if sel { pt.circle_filled(rcen, 5.0, ACCENT); }
                        if r.clicked() { self.cfg.lang = code.to_string(); ch = true; }
                        ui.add_space(2.0);
                    }
                }
            }
        });
        if ch {
            LANG.store(lang_idx(&self.cfg.lang), Ordering::Relaxed);
            write_cfg(&self.cfg);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let (vers, status, busy, progress, hits, acct, login, ended, launched, picked, meta, total) = {
            let mut s = self.sh.lock().unwrap();
            for h in s.results.iter_mut() {
                if let Some(ci) = h.icon.take() { self.tex.insert(h.slug.clone(), ctx.load_texture(h.slug.clone(), ci, Default::default())); }
            }
            if let Some(ci) = s.head.take() { self.head = Some(ctx.load_texture("head", ci, Default::default())); }
            if let Some((im, sl)) = s.guest_full.take() { self.guest = Some(mk_skin(ctx, "guest", &im, sl)); }
            if let Some((im, sl)) = s.acct_skin.take() { self.acct_skin = Some(mk_skin(ctx, "acct", &im, sl)); }
            for (n, im, sl) in std::mem::take(&mut s.defaults) { self.defs.push(mk_skin(ctx, &n, &im, sl)); }
            (s.versions.clone(), s.status.clone(), s.busy, s.progress, s.results.clone(), s.acct.clone(), s.login.clone(),
             std::mem::take(&mut s.ended), std::mem::replace(&mut s.launched, false), std::mem::take(&mut s.picked), s.meta.clone(), s.total)
        };
        self.has_acct = acct.is_some();
        let cin = self.sh.lock().unwrap().cloud_in.take();
        if let Some(v) = cin { self.apply_cloud(&v); }
        let nm = acct.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        if self.cfg.cloud_on && !nm.is_empty() && self.cloud_name != nm {
            if let Some((u, n, p)) = self.cloud_ready() { self.cloud_name = nm; cloud_pull(&self.sh, u, n, p); }
        }
        let now = ctx.input(|i| i.time);
        if self.page != self.pg_key { self.pg_key = self.page; self.pg_t0 = now; }
        let tt = ((now - self.pg_t0) / 0.18).clamp(0.0, 1.0) as f32;
        let ease = 1.0 - (1.0 - tt).powi(3);
        if tt < 1.0 { ctx.request_repaint(); }
        if launched && self.cfg.minimize { ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)); }
        if !ended.is_empty() {
            for (name, secs) in ended { if let Some(i) = self.insts.iter_mut().find(|i| i.name == name) { i.played += secs; } }
            save_insts(&self.insts);
            self.cloud_up();
            if self.cfg.refocus { ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false)); ctx.send_viewport_cmd(egui::ViewportCommand::Focus); }
        }
        for (kind, path) in picked { self.import_file(kind, &path); }
        if self.page == Page::Skins {
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            for d in dropped { if let Some(p) = d.path { self.import_file(0, &p); } }
        }
        if self.skins_dirty { self.reload_skins(ctx); }
        let eff: u8 = match self.cfg.theme { 3 => if self.sys_light { 1 } else { 0 }, t => t };
        if self.applied != eff as i16 { self.pal = pal_for(eff); apply_visuals(ctx, &self.pal); self.applied = eff as i16; }
        let p = self.pal;

        egui::SidePanel::left("nav").exact_width(72.0).resizable(false)
            .frame(egui::Frame::none().fill(p.side).inner_margin(egui::Margin::symmetric(13.0, 14.0)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if side_icon(ui, "home", self.page == Page::Home, &p).on_hover_text(tr("Home")).clicked() { self.page = Page::Home; }
                    if self.cfg.side_skins && side_icon(ui, "skins", self.page == Page::Skins, &p).on_hover_text(tr("Skins")).clicked() { self.page = Page::Skins; }
                    if side_icon(ui, "discover", self.page == Page::Discover, &p).on_hover_text(tr("Discover content")).clicked() { self.page = Page::Discover; self.want_search = true; }
                    if self.cfg.side_shots && side_icon(ui, "shots", self.page == Page::Shots, &p).on_hover_text("Screenshots").clicked() { self.page = Page::Shots; }
                    divider(ui, &p);
                    if side_icon(ui, "plus", self.page == Page::Create, &p).on_hover_text(tr("Create instance")).clicked() { self.page = Page::Create; }
                    for idx in self.order().into_iter().take(self.cfg.quick as usize) {
                        let i = self.insts[idx].clone();
                        if inst_icon(ui, &i, self.page == Page::Inst(idx), &p).on_hover_text(i.name.clone()).clicked() { self.page = Page::Inst(idx); }
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if side_icon(ui, "logout", false, &p).on_hover_text(tr("Logout")).clicked() {
                        let _ = fs::remove_file(root().join("account.txt"));
                        self.sh.lock().unwrap().acct = None; self.head = None; self.acct_skin = None;
                    }
                    if side_icon(ui, "settings", self.page == Page::Settings, &p).on_hover_text(tr("Settings")).clicked() { self.page = Page::Settings; }
                    divider(ui, &p);
                });
            });

        egui::TopBottomPanel::top("top").frame(egui::Frame::none().fill(p.bg).inner_margin(egui::Margin::symmetric(22.0, 10.0))).show(ctx, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let name = acct.as_ref().map_or(tr("Guest").to_string(), |a| a.name.clone());
                let shown: String = if name.chars().count() > 11 { name.chars().take(10).collect::<String>() + ".." } else { name };
                let (rc, r) = ui.allocate_exact_size(egui::vec2(176.0, 40.0), egui::Sense::click());
                let ht = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.12);
                ui.painter().rect_filled(rc, 6.0, mix(p.card, p.hover, ht));
                ui.painter().rect_stroke(rc, 6.0, egui::Stroke::new(1.0, p.stroke));
                self.paint_head(ui.painter(), egui::Rect::from_min_size(rc.min + egui::vec2(8.0, 6.0), egui::vec2(28.0, 28.0)), acct.is_some());
                ui.painter().text(egui::pos2(rc.left() + 46.0, rc.center().y), egui::Align2::LEFT_CENTER, shown, egui::FontId::proportional(15.0), ui.visuals().strong_text_color());
                let cc = egui::pos2(rc.right() - 16.0, rc.center().y);
                ui.painter().add(egui::Shape::line(vec![cc + egui::vec2(-4.0, -2.0), cc + egui::vec2(0.0, 2.0), cc + egui::vec2(4.0, -2.0)], egui::Stroke::new(1.5, ui.visuals().text_color())));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { self.acct_menu = !self.acct_menu; }
                self.chip_rect = rc;
            });
        });
        if self.acct_menu {
            let ar = egui::Area::new(egui::Id::new("acctmenu")).order(egui::Order::Foreground)
                .fixed_pos(self.chip_rect.right_bottom() + egui::vec2(-276.0, 8.0)).show(ctx, |ui| {
                    egui::Frame::none().fill(p.card).stroke(egui::Stroke::new(1.0, p.stroke)).rounding(8.0).inner_margin(egui::Margin::same(14.0))
                        .shadow(egui::Shadow { offset: egui::vec2(0.0, 6.0), blur: 18.0, spread: 0.0, color: Color32::from_black_alpha(110) })
                        .show(ui, |ui| { ui.set_width(248.0); self.account_menu(ui, &p, &acct, &login); });
                });
            if ctx.input(|i| i.pointer.any_click()) {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    if !ar.response.rect.contains(pos) && !self.chip_rect.contains(pos) { self.acct_menu = false; }
                }
            }
        }
        egui::TopBottomPanel::bottom("status").frame(egui::Frame::none().fill(p.side).inner_margin(egui::Margin::symmetric(14.0, 6.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if busy { ui.add(egui::ProgressBar::new(progress).desired_width(160.0).animate(true)); }
                    ui.label(RichText::new(&status).small().weak());
                });
            });
        let mut go: Option<usize> = None;
        egui::CentralPanel::default().frame(egui::Frame::none().fill(p.bg).inner_margin(egui::Margin::same(22.0))).show(ctx, |ui| {
            match self.page {
                Page::Home => go = self.page_home(ui, &p, &acct, busy),
                Page::Skins => self.page_skins(ui, &p),
                Page::Discover => self.page_discover(ui, &p, &hits, total),
                Page::Settings => self.page_settings(ui, &p),
                Page::Create => self.page_create(ui, &p, &vers),
                Page::Shots => self.page_shots(ui, &p),
                Page::Inst(i) => go = self.page_inst(ui, &p, i, busy),
            }
            if ease < 1.0 { ui.painter().rect_filled(ui.clip_rect(), 0.0, p.bg.linear_multiply(1.0 - ease)); }
        });
        if self.modal.is_some() { self.install_modal(ctx, &p, &meta); }
        if let Some(i) = go { self.start(i, &vers); }
    }
}

fn main() -> eframe::Result {
    let cfg = read_cfg();
    LANG.store(lang_idx(&cfg.lang), Ordering::Relaxed);
    let sh = Arc::new(Mutex::new(Shared { status: "Loading versions...".into(), ..Default::default() }));
    let s2 = sh.clone();
    std::thread::spawn(move || match load_versions() {
        Ok(v) => { let mut s = s2.lock().unwrap(); s.versions = v; s.status = "Ready!".into(); }
        Err(e) => set(&s2, &format!("Error: {e}")),
    });
    if let Some(a) = load_acct() {
        load_head(&sh, if a.refresh == "offline" { a.name.clone() } else { a.uuid.clone() });
        sh.lock().unwrap().acct = Some(a);
    }
    load_guest(&sh);
    let ico = image::load_from_memory(include_bytes!("../assets/icon.png")).unwrap().to_rgba8();
    let pwi = image::load_from_memory(include_bytes!("../assets/pw.png")).unwrap().to_rgba8();
    let (iw, ih) = (ico.width(), ico.height());
    let icon = egui::IconData { width: iw, height: ih, rgba: ico.into_raw() };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1240.0, 720.0]).with_min_inner_size([1000.0, 600.0])
            .with_title("Project Warden").with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("Project Warden", opts, Box::new(move |cc| {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert("cg".into(), egui::FontData::from_static(include_bytes!("../assets/CursorGothic-Regular.ttf")));
        fonts.font_data.insert("cgb".into(), egui::FontData::from_static(include_bytes!("../assets/CursorGothic-Bold.ttf")));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "cg".into());
        let mut bold = fonts.families[&egui::FontFamily::Proportional].clone();
        bold.insert(0, "cgb".into());
        fonts.families.insert(egui::FontFamily::Name("bold".into()), bold);
        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx.style_mut(|s| { s.spacing.item_spacing = egui::vec2(10.0, 10.0); s.spacing.button_padding = egui::vec2(14.0, 8.0); });
        let pw = cc.egui_ctx.load_texture("pw", egui::ColorImage::from_rgba_unmultiplied([pwi.width() as _, pwi.height() as _], &pwi), Default::default());
        let sys_light = cfg.theme == 3 && system_light();
        Ok(Box::new(App {
            sh, cfg, page: Page::Home, pal: pal_for(0), applied: -1, sys_light, has_acct: false,
            insts: load_insts(), newname: String::new(), newver: 0, newram: 4096, newkind: 1, cid: CLIENT_ID.to_string(),
            uname: String::new(), right_open: false, pg_key: Page::Home, pg_t0: 0.0, modal_t0: 0.0, cloud_name: String::new(), acct_menu: false, chip_rect: egui::Rect::NOTHING, icon_edit: false, itab: 0, stab: 0,
            dtype: 1, query: String::new(), dpage: 0, want_search: false, modal: None, minst: String::new(),
            yaw: 0.5,
            skins: vec![], capes: vec![], defs: vec![], guest: None, acct_skin: None, skins_dirty: true,
            pw, head: None, tex: HashMap::new(),
        }))
    }))
}
