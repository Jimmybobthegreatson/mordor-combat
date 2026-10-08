//! `som`: inspect a Shadow of Mordor install.

use std::io::Write;

use anyhow::{Context, Result, bail};
use som_formats::bndl::parse_manifest;
use som_formats::normalize_path;
use som_formats::vfs::Vfs;

const USAGE: &str = "\
usage: som <command>

  archives                 list mounted archives and their entries
  bundle <arch> <embb>     list the records of one bundle (arch = file name, e.g. corecharacter.arch05)
  ls [substring]           list asset paths known to the bundle manifests
  cat <path> [-o file]     write one asset to a file (or hex-dump its first bytes)
  clips <path>             list the clips of an animation bank (duration, codecs, root travel)
  db <path> [type [item]]  dump a GADB file: its types, a type's items, or one item
  prefab <path> [out.obj]  list a baked prefab (.inst): meshes, levels of detail, materials
  mat <path>               show a material's shader, textures and parameters
  verify [arch]            walk every bundle and compare it against its manifest
  grep <text> [arch ...]   find bundle records containing the text (case-insensitive ASCII)

The install is found from SOM_DIR, or by walking up from the current directory.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let mut vfs = Vfs::open(Vfs::locate()?)?;
    match command.as_str() {
        "archives" => archives(&vfs),
        "bundle" => {
            let [_, arch, embb] = args.as_slice() else { bail!("{USAGE}") };
            bundle(&mut vfs, arch, embb)
        }
        "audit" => {
            // som audit [out.tsv]: every attack the player's combat nodes define, checked against the clips the game loads.
            use som_formats::animator::Library;
            use som_formats::skel::Skeleton;
            use std::collections::{BTreeMap, HashMap};
            let db = som_formats::gamedb::GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let moves = som_formats::combatnodes::load(&db)?;
            let skeleton = Skeleton::parse(&vfs.read("models/player/player_talion/player_talion_pd.skel")?)?;
            let mut lib = Library::new(&skeleton)?;
            for bank in som_formats::banks::player_combat() {
                lib.add_bank(&vfs.read(&bank)?)?;
            }
            // Clip names of the orc banks (victim halves), and of every other bank on disk (to say where a missing clip lives).
            let names_of = |vfs: &mut Vfs, path: &str| -> Vec<String> {
                vfs.read(path).ok().and_then(|d| som_formats::anix::Anix::parse(&d).ok()).map(|a| a.clips.into_iter().map(|c| c.name).collect()).unwrap_or_default()
            };
            let mut orc: HashMap<String, String> = HashMap::new();
            for bank in som_formats::banks::ORC {
                for n in names_of(&mut vfs, bank) {
                    orc.entry(n).or_insert_with(|| bank.to_string());
                }
            }
            let mut elsewhere: HashMap<String, String> = HashMap::new();
            for path in vfs.paths()? {
                if path.ends_with(".anix") && path.starts_with("animation/") && !path.contains("dlc") {
                    for n in names_of(&mut vfs, &path) {
                        elsewhere.entry(n).or_insert_with(|| path.clone());
                    }
                }
            }
            let mut rows: Vec<String> = vec!["node\tattack\tclip\tstatus\tdur\tscale\tBCOL\tAD\tECOL\tBUFFTRANS\ttravel_at_blow_cm\tyaw_deg\tstance\tsides\trange\tissues".into()];
            let mut per_node: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
            let mut issue_count: BTreeMap<String, usize> = BTreeMap::new();
            let mut wanted_banks: BTreeMap<String, usize> = BTreeMap::new();
            for node in &moves.nodes {
                let mut seen = std::collections::HashSet::new();
                for &i in &node.attacks {
                    let a = &moves.attacks[i];
                    if !seen.insert(a.name.clone()) {
                        continue;
                    }
                    let entry = per_node.entry(node.name.clone()).or_default();
                    entry.0 += 1;
                    let mut issues: Vec<String> = Vec::new();
                    let sync = a.victim_clip.is_some();
                    let (status, detail) = match lib.find(&a.clip) {
                        None => {
                            match elsewhere.get(&a.clip) {
                                Some(bank) => {
                                    *wanted_banks.entry(bank.clone()).or_default() += 1;
                                    issues.push(format!("clip in unloaded bank {bank}"));
                                }
                                None => issues.push("clip in no bank on disk".into()),
                            }
                            ("MISSING", String::from("\t\t\t\t\t\t\t"))
                        }
                        Some(clip) => {
                            entry.1 += 1;
                            let dur = lib.duration_ms(clip) / 1000.0;
                            let scale = lib.cue_scale(clip);
                            let ev = |n: &str| lib.event_s(clip, n);
                            let (bcol, ad, ecol, buff) = (ev("BCOL"), ev("APPLYDAMAGE"), ev("ECOL"), ev("BUFFTRANS"));
                            if !sync {
                                if bcol.is_none() {
                                    issues.push("no BCOL".into());
                                }
                                if ad.is_none() {
                                    issues.push("no APPLYDAMAGE/AD".into());
                                }
                                if let (Some(b), Some(d)) = (bcol, ad) {
                                    if b > d + 0.02 {
                                        issues.push("BCOL after damage cue".into());
                                    }
                                }
                                if buff.is_none() && ev("EHITWIN").is_none() {
                                    issues.push("no BUFFTRANS/EHITWIN".into());
                                }
                            }
                            for (n, t) in [("BCOL", bcol), ("AD", ad)] {
                                if t.is_some_and(|t| t > dur + 0.02) {
                                    issues.push(format!("{n} past clip end"));
                                }
                            }
                            let blow = bcol.or(ad).unwrap_or(dur * 0.5);
                            let travel = lib
                                .sample(clip, lib.start_ms(clip) + blow * 1000.0)
                                .map(|skin| {
                                    let r = skin[lib.root_bone()].t;
                                    r[0].hypot(r[2])
                                })
                                .unwrap_or(0.0);
                            let yaw = lib.root_total(clip).map(|(_, y)| y.to_degrees()).unwrap_or(0.0);
                            // The name encodes distance class, side and stances: ..._D1_F_LL_RR_...
                            let parts: Vec<&str> = a.clip.split('_').collect();
                            if let Some(d) = parts.iter().position(|p| p.len() == 2 && p.starts_with('D') && p[1..].chars().all(|c| c.is_ascii_digit())) {
                                let side = parts.get(d + 1).copied().unwrap_or("");
                                let word = match side {
                                    "F" => "Forward",
                                    "B" => "Back",
                                    "L" => "Left",
                                    "R" => "Right",
                                    _ => "",
                                };
                                if !sync && !word.is_empty() && !a.allowed.is_empty() && !a.allowed.contains(word) {
                                    issues.push(format!("clip side {side} not in allowed {}", a.allowed));
                                }
                                let stance = |x: Option<&&str>| x.filter(|s| s.len() == 2 && s.chars().all(|c| c == 'L' || c == 'R')).map(|s| s.to_string());
                                if let (Some(s0), false) = (stance(parts.get(d + 2)), a.start.is_empty()) {
                                    if s0 != a.start {
                                        issues.push(format!("clip starts {s0}, data says {}", a.start));
                                    }
                                }
                                if let (Some(s1), false) = (stance(parts.get(d + 3)), a.end.is_empty()) {
                                    if s1 != a.end {
                                        issues.push(format!("clip ends {s1}, data says {}", a.end));
                                    }
                                }
                                // The clip's own turn should match the side it is authored for.
                                let turn = (yaw.rem_euclid(360.0) + 180.0).rem_euclid(360.0) - 180.0;
                                let expect: Option<f32> = match side {
                                    "F" => Some(0.0),
                                    "B" => Some(180.0),
                                    "L" => Some(-90.0),
                                    "R" => Some(90.0),
                                    _ => None,
                                };
                                if let (Some(e), false) = (expect, sync) {
                                    let off = ((turn - e + 180.0).rem_euclid(360.0) - 180.0).abs();
                                    if off > 50.0 {
                                        issues.push(format!("turns {turn:.0} deg for side {side}"));
                                    }
                                }
                            }
                            let f = |t: Option<f32>| t.map_or("-".to_string(), |t| format!("{t:.2}"));
                            ("ok", format!("{dur:.2}\t{scale:.2}\t{}\t{}\t{}\t{}\t{travel:.0}\t{yaw:.0}", f(bcol), f(ad), f(ecol), f(buff)))
                        }
                    };
                    if let Some(victim) = &a.victim_clip {
                        if !orc.contains_key(victim) && !orc.contains_key(&victim.replace("_NoSev", "")) {
                            match elsewhere.get(victim) {
                                Some(bank) => {
                                    *wanted_banks.entry(bank.clone()).or_default() += 1;
                                    issues.push(format!("victim clip in unloaded bank {bank}"));
                                }
                                None => issues.push("victim clip in no bank".into()),
                            }
                        }
                    }
                    if !issues.is_empty() {
                        entry.2 += 1;
                        for i in &issues {
                            let key = i.split(" animation/").next().unwrap_or(i).to_string();
                            *issue_count.entry(key).or_default() += 1;
                        }
                    }
                    rows.push(format!(
                        "{}\t{}\t{}\t{status}\t{detail}\t{}>{}\t{}/{}\t{:.0}-{:.0}\t{}",
                        node.name, a.name, a.clip, a.start, a.end, a.allowed, a.attacker_sides, a.range_cm.0, a.range_cm.1, issues.join("; ")
                    ));
                }
            }
            if let Some(out) = args.get(1) {
                std::fs::write(out, rows.join("\n"))?;
                println!("wrote {} rows to {out}", rows.len() - 1);
            }
            println!("{:<34} {:>7} {:>7} {:>7}", "node", "attacks", "loaded", "issues");
            for (n, (total, loaded, bad)) in &per_node {
                println!("{n:<34} {total:>7} {loaded:>7} {bad:>7}");
            }
            println!("\nissues:");
            for (i, n) in &issue_count {
                println!("  {n:>5}  {i}");
            }
            println!("\nbanks that hold missing clips:");
            for (b, n) in &wanted_banks {
                println!("  {n:>5}  {b}");
            }
            Ok(())
        }
        "findclip" => {
            // som findclip <name substring> [path substring]: which animation banks hold a clip.
            let needle = args.get(1).context(USAGE)?.to_ascii_lowercase();
            let scope = args.get(2).map(|s| normalize_path(s)).unwrap_or_else(|| "animation/".into());
            for path in vfs.paths()? {
                if !path.ends_with(".anix") || !path.contains(scope.as_str()) {
                    continue;
                }
                let Ok(data) = vfs.read(&path) else { continue };
                let Ok(anix) = som_formats::anix::Anix::parse(&data) else { continue };
                let hits: Vec<&str> = anix.clips.iter().map(|c| c.name.as_str()).filter(|n| n.to_ascii_lowercase().contains(&needle)).collect();
                if !hits.is_empty() {
                    println!("{path}: {} clips, e.g. {}", hits.len(), hits[0]);
                }
            }
            Ok(())
        }
        "ls" => ls(&mut vfs, args.get(1).map(String::as_str)),
        "cat" => {
            let path = args.get(1).context(USAGE)?;
            let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
            cat(&mut vfs, path, out.map(String::as_str))
        }
        "blocks" => {
            let anix = som_formats::anix::Anix::parse(&vfs.read(args.get(1).context(USAGE)?)?)?;
            let name = args.get(2).context(USAGE)?;
            let tag = args.get(3).context(USAGE)?;
            let clip = anix.clip(name).context("no such clip")?;
            for (t, elements, data) in anix.codec_blocks(clip)? {
                if &t != tag {
                    continue;
                }
                println!("{t}: {} elements", elements.len());
                for (c, n) in &elements {
                    println!("  channel {c:08x} node {n:08x}");
                }
                for (i, row) in data.chunks(16).take(24).enumerate() {
                    let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
                    let txt: String = row.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
                    println!("{:04x}  {:<48} {txt}", i * 16, hex.join(" "));
                }
            }
            Ok(())
        }
        "states" => {
            // som states <bvr> [substring]: behaviour states with the clip each plays and where each can go next.
            use som_formats::gamedb::{GameDb, Value};
            use som_formats::hash::name_hash;
            let db = GameDb::parse(vfs.read(args.get(1).context(USAGE)?)?)?;
            let filter = args.get(2).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
            let refs = |e: &som_formats::gamedb::Entry, field: &str| -> Vec<u32> {
                match e.field(name_hash(field)) {
                    Some(Value::Raw { kind: 10, values }) => values,
                    Some(Value::Refs(r)) => r,
                    _ => Vec::new(),
                }
            };
            let text = |e: &som_formats::gamedb::Entry, field: &str| match e.field(name_hash(field)) {
                Some(Value::Strings(s)) => s.join(","),
                _ => String::new(),
            };
            let ty = (0..db.type_count()).find(|&i| db.type_info(i).is_some_and(|t| t.0 == "BlendState")).context("no BlendState")?;
            let (_, f, r) = db.type_info(ty).unwrap();
            for i in f..f + r {
                let Some(e) = db.item(ty, i) else { continue };
                let name = e.name().unwrap_or_default();
                if !name.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                let clips: Vec<String> = refs(&e, "AnimationLink")
                    .iter()
                    .filter_map(|&v| db.follow(v))
                    .map(|a| {
                        let clip = text(&a, "animation");
                        if clip.is_empty() { format!("<{}>", a.name().unwrap_or_default()) } else { clip }
                    })
                    .collect();
                let links: Vec<String> = refs(&e, "Links")
                    .iter()
                    .filter_map(|&v| db.follow(v))
                    .map(|t| t.name().unwrap_or_default().split('.').skip(1).collect::<Vec<_>>().join("."))
                    .collect();
                println!("{name}\t{}\t{}", clips.join(","), links.join(","));
            }
            Ok(())
        }
        "stats" => {
            // Stat entries (key -> value) whose state modifiers touch the given states: how the game stuns / knocks down.
            use som_formats::gamedb::{GameDb, Value};
            use som_formats::hash::name_hash;
            let db = GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let needle = args.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "stun".into());
            let refs = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> Vec<u32> {
                match e.field(name_hash(n)) {
                    Some(Value::Refs(r)) | Some(Value::Raw { values: r, .. }) => r,
                    _ => Vec::new(),
                }
            };
            let names = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> Vec<String> {
                refs(e, n).iter().filter_map(|&r| db.follow(r)).filter_map(|x| x.name()).collect()
            };
            let float = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> f32 {
                match e.field(name_hash(n)) {
                    Some(Value::Raw { values, .. }) => f32::from_bits(values.first().copied().unwrap_or(0)),
                    _ => f32::NAN,
                }
            };
            let t = (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == "Combat/Stats/StatEntries")).unwrap();
            let (_, f, r) = db.type_info(t).unwrap();
            for i in f..f + r {
                let Some(e) = db.item(t, i) else { continue };
                let (Some(key), Some(val)) = (
                    refs(&e, "pattern").first().and_then(|&r| db.follow(r)),
                    refs(&e, "values").first().and_then(|&r| db.follow(r)),
                ) else {
                    continue;
                };
                let mut mods = Vec::new();
                for m in refs(&val, "StateMods") {
                    let Some(m) = db.follow(m) else { continue };
                    let state = names(&m, "State").join(",");
                    if !state.to_ascii_lowercase().contains(&needle) && needle != "all" {
                        continue;
                    }
                    let Some(modi) = refs(&m, "Modifier").first().and_then(|&r| db.follow(r)) else { continue };
                    mods.push(format!("{state} +{} x{}", float(&modi, "AdditiveModifier"), float(&modi, "MultiplicativeModifier")));
                }
                if mods.is_empty() {
                    continue;
                }
                let mut conds = Vec::new();
                for field in ["AttackerCategory", "DefenderCategory", "DirectionsAllowed", "CombatStateSelfList", "SelfHealthPct",
                    "AttackerActiveItem", "RequiredItem", "ExcludedItem", "TargetRequiredItem", "TargetExcludedItem", "Blocking", "SelfRecoilCounterRange", "AttackerCharacterTypeList"] {
                    let n = names(&key, field);
                    if !n.is_empty() {
                        conds.push(format!("{field}={}", n.join("|")));
                    }
                }
                println!("{} [{}] {:?}
    {}
    {}", e.name().unwrap_or_default(), i, names(&e, "Notes"), conds.join("  "), mods.join("; "));
            }
            Ok(())
        }
        "slowmo" => {
            // SlowMo records: period, ramps and the time scales (float pool), plus the player's combat slow-mo setup.
            use som_formats::gamedb::{GameDb, Value};
            use som_formats::hash::name_hash;
            let db = GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let filter = args.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
            let bits = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> f32 {
                match e.field(name_hash(n)) {
                    Some(Value::Raw { values, .. }) => f32::from_bits(values.first().copied().unwrap_or(0)),
                    _ => f32::NAN,
                }
            };
            let pool = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> Vec<f32> {
                e.raw_field(name_hash(n)).unwrap_or(&[]).iter().filter_map(|&o| db.pool_f32(o)).collect()
            };
            let t = (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == "SlowMo")).unwrap();
            let (_, f, r) = db.type_info(t).unwrap();
            for i in f..f + r {
                let Some(e) = db.item(t, i) else { continue };
                let name = e.name().unwrap_or_default();
                if !name.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                println!("{name:<32} period {:.2} up {:.2} down {:.2} sim {:?} player {:?}", bits(&e, "Period"), bits(&e, "RampUp"), bits(&e, "RampDown"),
                    pool(&e, "SimulationTimeScale"), pool(&e, "PlayerTimeScale"));
            }
            Ok(())
        }
        "durations" => {
            // AppliedState records: state, base chance and the float(s) behind `Duration` / its overrides.
            use som_formats::gamedb::{GameDb, Value};
            use som_formats::hash::name_hash;
            let db = GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let t = (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == "Combat/States/AppliedState")).unwrap();
            let (_, f, r) = db.type_info(t).unwrap();
            let floats = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> Vec<f32> {
                e.raw_field(name_hash(n)).unwrap_or(&[]).iter().filter_map(|&o| db.pool_f32(o)).collect()
            };
            for i in f..f + r {
                let Some(e) = db.item(t, i) else { continue };
                let state: Vec<String> = match e.field(name_hash("State")) {
                    Some(Value::Refs(r)) => r.iter().filter_map(|&r| db.follow(r)).filter_map(|x| x.name()).collect(),
                    _ => vec![],
                };
                let mut over = Vec::new();
                if let Some(Value::Refs(rs)) = e.field(name_hash("DurationOverrides")) {
                    for o in rs.iter().filter_map(|&r| db.follow(r)) {
                        let item: Vec<String> = match o.field(name_hash("RequiredItem")) {
                            Some(Value::Refs(r)) => r.iter().filter_map(|&r| db.follow(r)).filter_map(|x| x.name()).collect(),
                            _ => vec![],
                        };
                        over.push(format!("{:?}->{:?}", item, floats(&o, "Duration")));
                    }
                }
                println!("{:<40} {:<14} duration {:?} raw {:?} overrides {}", e.name().unwrap_or_default(), state.join(","), floats(&e, "Duration"),
                    e.raw_field(name_hash("Duration")), over.join(" "));
            }
            Ok(())
        }
        "applied" => {
            // What each attack category does to its victim: AppliedState records (state, base chance).
            use som_formats::gamedb::{GameDb, Value};
            use som_formats::hash::name_hash;
            let db = GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let prefix = args.get(1).cloned().unwrap_or_else(|| "PC_".into());
            let refs = |e: &som_formats::gamedb::Entry<'_>, n: &str| -> Vec<u32> {
                match e.field(name_hash(n)) {
                    Some(Value::Refs(r)) | Some(Value::Raw { values: r, .. }) => r,
                    _ => Vec::new(),
                }
            };
            let t = (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == "Combat/Category")).unwrap();
            let (_, f, r) = db.type_info(t).unwrap();
            for i in f..f + r {
                let Some(e) = db.item(t, i) else { continue };
                let name = e.name().unwrap_or_default();
                if !name.starts_with(&prefix) {
                    continue;
                }
                for field in ["AppliedState", "AppliedStateOnCrit"] {
                    for a in refs(&e, field).iter().filter_map(|&r| db.follow(r)) {
                        let chance = match a.field(name_hash("BaseChance")) {
                            Some(Value::Raw { values, .. }) => f32::from_bits(values.first().copied().unwrap_or(0)),
                            _ => f32::NAN,
                        };
                        let state: Vec<String> =
                            refs(&a, "State").iter().filter_map(|&r| db.follow(r)).filter_map(|x| x.name()).collect();
                        println!("{name} {field} {} -> {} chance {chance}", a.name().unwrap_or_default(), state.join(","));
                    }
                }
            }
            Ok(())
        }
        "skel" => {
            let sk = som_formats::skel::Skeleton::parse(&vfs.read(args.get(1).context(USAGE)?)?)?;
            for (i, b) in sk.bones.iter().enumerate() {
                println!("{i} {} parent={:?}", b.name, b.parent);
            }
            Ok(())
        }
        "clips" => clips(&mut vfs, args.get(1).context(USAGE)?),
        "attacks" => {
            let db = som_formats::gamedb::GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            for a in som_formats::combatdb::pc_attacks(&db) {
                println!(
                    "{}	{}	{}-{}	{}	{}>{}	{}	D{} {}..{}	{}	{}",
                    a.name, a.pool, a.hits.0, a.hits.1, a.clip, a.start, a.end, a.allowed, a.distance, a.range_cm.0, a.range_cm.1, a.kind, a.scaling
                );
            }
            Ok(())
        }
        "syncfit" => {
            use som_formats::animator::Library;
            use som_formats::skel::Skeleton;
            // som syncfit <player bank> <player clip prefix> <victim bank> <victim clip prefix> [suffix ...]
            let (pb, pp, vb, vp) = (
                args.get(1).context(USAGE)?,
                args.get(2).context(USAGE)?,
                args.get(3).context(USAGE)?,
                args.get(4).context(USAGE)?,
            );
            let pskel = Skeleton::parse(&vfs.read("models/player/player_talion/player_talion_pd.skel")?)?;
            let vskel = Skeleton::parse(&vfs.read("models/npc/orcs/orc_defender.skel")?)?;
            let mut plib = Library::new(&pskel)?;
            let pbank = plib.add_bank(&vfs.read(pb)?)?;
            let mut vlib = Library::new(&vskel)?;
            let vbank = vlib.add_bank(&vfs.read(vb)?)?;
            let hand = plib.find_bone("R_Weapon").context("no R_Weapon")?;
            let torso = vlib.find_bone("Torso_Base").or_else(|| vlib.find_bone("Pelvis")).context("no torso")?;
            for suffix in &args[5..] {
                let (Some(pc), Some(vc)) = (plib.clip(pbank, &format!("{pp}{suffix}")), vlib.clip(vbank, &format!("{vp}{suffix}"))) else {
                    println!("{suffix}: clip missing");
                    continue;
                };
                let hit = plib.event_s(pc, "BHITWIN").or_else(|| plib.event_s(pc, "APPLYDAMAGE")).unwrap_or(plib.duration_ms(pc) / 1000.0 * 0.6);
                let window = ((hit - 0.5).max(0.0), hit);
                let front = !suffix.starts_with('B');
                match som_formats::sync::fit_near(&plib, pc, hand, &vlib, vc, torso, 70.0, window, front, 110.0, 60.0) {
                    Some(f) => println!(
                        "{suffix:<28} window {:.2}-{:.2}s  victim at ({:+.0},{:+.0}) cm  yaw {:+.0} deg  residual {:.0} cm",
                        window.0, window.1, f.offset[0], f.offset[1], f.yaw.to_degrees(), f.residual
                    ),
                    None => println!("{suffix}: no fit"),
                }
            }
            Ok(())
        }
        "nodes" => {
            // som nodes [prefix]: Combat/Node records with priority, input predicate, alternate node and pools.
            use som_formats::gamedb::{GameDb, Value};
            let db = GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let prefix = args.get(1).map(String::as_str).unwrap_or("PC_");
            let t = (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == "Combat/Node")).context("no Combat/Node")?;
            let (_, f, r) = db.type_info(t).unwrap();
            let name_of = |v: u32| db.follow(v).and_then(|e| e.name()).unwrap_or_default();
            for i in f..f + r {
                let Some(e) = db.item(t, i) else { continue };
                let n = e.name().unwrap_or_default();
                if !n.starts_with(prefix) {
                    continue;
                }
                let get = |h: u32| e.field(h);
                let prio = match get(0x9e810eaa) { Some(Value::Raw { values, .. }) => values.first().copied().unwrap_or(0), _ => 0 };
                let refs = |h: u32| -> Vec<String> {
                    match get(h) {
                        Some(Value::Refs(r)) | Some(Value::Raw { values: r, .. }) => r.iter().map(|&v| name_of(v)).filter(|s| !s.is_empty()).collect(),
                        _ => Vec::new(),
                    }
                };
                let input = e
                    .field(0xf6bb663f)
                    .and_then(|v| match v { Value::Refs(r) => r.first().copied(), _ => None })
                    .and_then(|r| db.follow(r))
                    .map(|p| match p.field(0x3372b912) { Some(Value::Raw { values, .. }) => values.first().map(|&v| name_of(v)).unwrap_or_default(), _ => String::new() })
                    .unwrap_or_default();
                println!("{n}	prio {prio}	input {input}	alt {:?}	next {:?}	pools {:?}", refs(0xad5b9a78), refs(0x4a4ddc8e), refs(0xe6a2e99f));
            }
            Ok(())
        }
        "pastend" => {
            // som pastend <bank> <clip>: does the pose keep changing after the clip's nominal end?
            let data = vfs.read(args.get(1).context(USAGE)?)?;
            let anix = som_formats::anix::Anix::parse(&data)?;
            let clip = anix.clip(args.get(2).context(USAGE)?).context("no such clip")?;
            let bind = anix.bind_pose()?;
            let end = clip.end_ms();
            let mut prev: Option<som_formats::anix::Pose> = None;
            for t in (0..(end * 3).max(4000)).step_by(250) {
                let pose = anix.sample_unclamped(clip, t, &bind)?;
                let diff = prev.as_ref().map(|p| p.rotation.iter().zip(&pose.rotation).map(|(a, b)| 1.0 - (a[0]*b[0]+a[1]*b[1]+a[2]*b[2]+a[3]*b[3]).abs()).sum::<f32>()).unwrap_or(0.0);
                println!("t {t:>5} ms {}  change vs previous {diff:.4}", if t > end { "(past end)" } else { "" });
                prev = Some(pose);
            }
            Ok(())
        }
        "moves" => {
            // som moves [node prefix]: the selection data per node: input, priority and attacks with their predicates.
            let db = som_formats::gamedb::GameDb::parse(vfs.read("database/game/game.gamedb")?)?;
            let moves = som_formats::combatnodes::load(&db)?;
            let prefix = args.get(1).map(String::as_str).unwrap_or("");
            for n in moves.nodes.iter().filter(|n| n.name.starts_with(prefix)) {
                println!("{}  input {}  prio {}  ({} attacks)", n.name, n.input, n.priority, n.attacks.len());
                for &a in n.attacks.iter().take(if prefix.is_empty() { 0 } else { 2000 }) {
                    let d = &moves.attacks[a];
                    println!(
                        "    {:<44} clip {:<40} victim {:<40} val {:<5} item {:<18} states {:<28} parry {:<14} sides {}/{} range {:.0}-{:.0} hits {:?} {}>{} health {:?} needs [{}] forbids [{}] target needs [{}] forbids [{}] applied {:?}",
                        d.name, d.clip, d.victim_clip.as_deref().unwrap_or("-"), d.explicit, d.item, d.target_states, d.parry,
                        d.allowed, d.attacker_sides, d.range_cm.0, d.range_cm.1, d.hits, d.start, d.end, d.health_range, d.needs_item, d.forbids_item, d.target_needs, d.target_forbids,
                        d.applied.iter().map(|a| format!("{} {:.2} {:.1}s hc{}-{}", a.state, a.chance, a.duration, a.hits.0, a.hits.1.min(999))).collect::<Vec<_>>()
                    );
                }
            }
            Ok(())
        }
        "dbfind" => {
            use som_formats::gamedb::{GameDb, Value};
            let db = GameDb::parse(vfs.read(args.get(1).context(USAGE)?)?)?;
            let needle = args.get(2).context(USAGE)?.to_ascii_lowercase();
            for t in 0..db.type_count() {
                let (tn, f, r) = db.type_info(t).unwrap();
                for i in 0..f + r {
                    let Some(e) = db.item(t, i) else { continue };
                    for c in e.cells() {
                        if let Value::Strings(v) = e.value(c) {
                            if v.iter().any(|s| s.to_ascii_lowercase().contains(&needle)) {
                                println!("{tn} {i} {} : {:x} = {v:?}", e.name().unwrap_or_default(), c.hash);
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        "valmods" => {
            // som valmods <value name>: the links (StateTransition) whose ValueModifiers set the value, with the number.
            use som_formats::gamedb::{GameDb, Value};
            use som_formats::hash::name_hash;
            let db = GameDb::parse(vfs.read("behaviors/player/player_elf.bvr")?)?;
            let want = args.get(1).context(USAGE)?;
            let st = (0..db.type_count()).find(|&i| db.type_info(i).is_some_and(|t| t.0 == "StateTransition")).context("no StateTransition")?;
            let (_, f, r) = db.type_info(st).unwrap();
            let first = |e: &som_formats::gamedb::Entry<'_>, field: &str| match e.field(name_hash(field)) {
                Some(Value::Raw { values, .. }) => values.first().copied(),
                Some(Value::Refs(v)) => v.first().copied(),
                _ => None,
            };
            let mut n = 0;
            for i in 0..f + r {
                let Some(e) = db.item(st, i) else { continue };
                let Some(Value::Refs(list)) = e.field(name_hash("ValueModifiers")) else { continue };
                for m in list {
                    let Some(m) = db.follow(m) else { continue };
                    let Some(node) = first(&m, "ValueNode").and_then(|v| db.follow(v)) else { continue };
                    if node.name().as_deref() != Some(want.as_str()) {
                        continue;
                    }
                    n += 1;
                    let num = first(&m, "Number").and_then(|v| db.follow(v)).and_then(|x| x.name());
                    if n <= 60 {
                        println!("{} = {:?}", e.name().unwrap_or_default(), num);
                    }
                }
            }
            println!("{n} modifiers");
            Ok(())
        }
        "hash" => {
            for n in &args[1..] {
                println!("{:x} {n}", som_formats::hash::name_hash(n));
            }
            Ok(())
        }
        "graph" => {
            // som graph <state>: the state as the runner parses it (clip tree, rate, transitions with parsed predicates).
            let g = som_formats::bvrgraph::Graph::parse(vfs.read("behaviors/player/player_elf.bvr")?)?;
            let st = g.state(args.get(1).context(USAGE)?).context("no such state")?;
            println!("anim {:?}
rate {:?}", st.anim, st.rate);
            for t in &st.transitions {
                println!("{} -> {} [{}..{}] blend {} {:?}", t.name, t.target, t.min, t.max, t.blend, t.pred);
            }
            Ok(())
        }
        "db" => db(&mut vfs, args.get(1).context(USAGE)?, args.get(2), args.get(3)),
        "verify" => verify(&mut vfs, args.get(1).map(String::as_str)),
        "grep" => grep(&mut vfs, args.get(1).context(USAGE)?, &args[2..]),
        _ => bail!("{USAGE}"),
    }
}

fn archives(vfs: &Vfs) -> Result<()> {
    for arch in &vfs.archives {
        println!("{}", arch.path.file_name().unwrap().to_string_lossy());
        for entry in &arch.entries {
            println!("  {:>12}  {}", entry.size, entry.name);
        }
    }
    Ok(())
}

fn archive_index(vfs: &Vfs, name: &str) -> Result<usize> {
    vfs.archives
        .iter()
        .position(|a| a.path.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name)))
        .with_context(|| format!("archive {name} is not mounted"))
}

fn bundle(vfs: &mut Vfs, arch: &str, embb: &str) -> Result<()> {
    let index = archive_index(vfs, arch)?;
    let bundle = vfs.bundle(index, embb)?;
    let mut out = std::io::stdout().lock();
    for record in &bundle.records {
        writeln!(out, "{:>10}  {}", record.size, record.name)?;
    }
    Ok(())
}

fn ls(vfs: &mut Vfs, filter: Option<&str>) -> Result<()> {
    let filter = filter.map(normalize_path);
    let mut out = std::io::stdout().lock();
    for path in vfs.paths()? {
        if filter.as_ref().is_none_or(|f| path.contains(f.as_str())) {
            writeln!(out, "{path}")?;
        }
    }
    Ok(())
}

fn cat(vfs: &mut Vfs, path: &str, out: Option<&str>) -> Result<()> {
    let data = vfs.read(path)?;
    match out {
        Some(file) => {
            std::fs::write(file, &data)?;
            println!("{} bytes -> {file}", data.len());
        }
        None => {
            println!("{} bytes", data.len());
            hexdump(&data[..data.len().min(256)]);
        }
    }
    Ok(())
}

fn hexdump(data: &[u8]) {
    for (i, row) in data.chunks(16).enumerate() {
        let hex: String = row.iter().map(|b| format!("{b:02x} ")).collect();
        let text: String =
            row.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }).collect();
        println!("{:06x}  {hex:<48} {text}", i * 16);
    }
}

/// Every record must be listed in the manifest with the same size.
fn verify(vfs: &mut Vfs, only: Option<&str>) -> Result<()> {
    let only = only.map(|name| archive_index(vfs, name)).transpose()?;
    let (mut bundles, mut records, mut problems) = (0usize, 0usize, 0usize);
    for ai in 0..vfs.archives.len() {
        if only.is_some_and(|o| o != ai) {
            continue;
        }
        let arch_name = vfs.archives[ai].path.file_name().unwrap().to_string_lossy().into_owned();
        let entries: Vec<String> = vfs.archives[ai].entries.iter().map(|e| e.name.clone()).collect();
        for name in entries.iter().filter(|n| n.to_ascii_lowercase().ends_with(".embb")) {
            let manifest_name = format!("{}.bndlxml05", &name[..name.len() - 5]);
            let arch = &vfs.archives[ai];
            let manifest: std::collections::HashMap<String, Option<u64>> = match arch.find(&manifest_name) {
                Some(entry) => parse_manifest(&arch.read(entry)?)
                    .into_iter()
                    .map(|(n, s)| (normalize_path(&n), s))
                    .collect(),
                None => Default::default(),
            };
            let bundle = match vfs.bundle(ai, name) {
                Ok(bundle) => bundle,
                Err(err) => {
                    println!("FAIL {arch_name}:{name}: {err:#}");
                    problems += 1;
                    continue;
                }
            };
            let mut bad = 0;
            for record in &bundle.records {
                match manifest.get(&normalize_path(&record.name)) {
                    Some(Some(size)) if *size == record.size as u64 => {}
                    Some(None) => {}
                    _ if manifest.is_empty() => {}
                    _ => bad += 1,
                }
            }
            println!(
                "{} {arch_name}:{name}: {} records, {} manifest entries, {bad} mismatched",
                if bad == 0 { "ok  " } else { "DIFF" },
                bundle.records.len(),
                manifest.len()
            );
            bundles += 1;
            records += bundle.records.len();
            problems += (bad > 0) as usize;
        }
    }
    println!("{bundles} bundles, {records} records, {problems} with problems");
    Ok(())
}

/// Which records mention `text`? Useful for finding the file that ties two assets together.
fn grep(vfs: &mut Vfs, text: &str, only: &[String]) -> Result<()> {
    let needle = text.to_ascii_lowercase().into_bytes();
    for ai in 0..vfs.archives.len() {
        let arch_name = vfs.archives[ai].path.file_name().unwrap().to_string_lossy().into_owned();
        if !only.is_empty() && !only.iter().any(|o| o.eq_ignore_ascii_case(&arch_name)) {
            continue;
        }
        let entries: Vec<String> = vfs.archives[ai].entries.iter().map(|e| e.name.clone()).collect();
        for name in entries.iter().filter(|n| n.to_ascii_lowercase().ends_with(".embb")) {
            let Ok(bundle) = vfs.bundle(ai, name) else { continue };
            for record in bundle.records.clone() {
                // Textures and audio never hold references; skipping them keeps this fast.
                let lower = record.name.to_ascii_lowercase();
                if lower.ends_with(".tex") || lower.ends_with(".bnk") || lower.ends_with(".anix") {
                    continue;
                }
                let data = bundle.read(&record)?;
                let hit = data.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(&needle));
                if hit {
                    println!("{arch_name}:{name}: {} ({} bytes)", record.name, record.size);
                }
            }
        }
    }
    Ok(())
}

fn clips(vfs: &mut Vfs, path: &str) -> Result<()> {
    use som_formats::animator::{Animator, Library};
    use som_formats::skel::Skeleton;
    let skel_path = std::env::var("SOM_SKEL").unwrap_or_else(|_| "models/player/player_talion/player_talion_pd.skel".into());
    let skeleton = Skeleton::parse(&vfs.read(&skel_path)?)?;
    let mut lib = Library::new(&skeleton)?;
    let bank = lib.add_bank(&vfs.read(path)?)?;
    let anix = som_formats::anix::Anix::parse(&vfs.read(path)?)?;
    println!("{} groups", anix.clips.len());
    for clip in &anix.clips {
        let Some(id) = lib.clip(bank, &clip.name).or_else(|| lib.clip(bank, &clip.name)) else { continue };
        // Play it once at 60 Hz and total up the root motion.
        let mut animator = Animator::new();
        let mut stats = String::new();
        if clip.duration_ms() > 0 && animator.play(&lib, id, false, 0.0, 0.0).is_ok() {
            let (mut t, mut yaw, mut top) = ([0.0f32; 3], 0.0f32, 0.0f32);
            for _ in 0..(clip.duration_ms() as f32 * 0.06).ceil() as usize {
                let Ok(step) = animator.update(&lib, 1.0 / 60.0) else { break };
                for k in 0..3 {
                    t[k] += step.travel[k];
                }
                yaw += step.yaw;
                top = top.max(t[1]);
            }
            if t[0].abs() + t[2].abs() > 5.0 || yaw.abs() > 0.05 || top > 5.0 {
                stats = format!("  dx {:+.0} dz {:+.0}  yaw {:+.0}°  peak y {:.0} end y {:.0}", t[0], t[2], yaw.to_degrees(), top, t[1]);
            }
        }
        if std::env::var_os("SOM_ROOT0").is_some() {
            if let Ok(skin) = lib.sample(id, lib.start_ms(id)) {
                let r = skin[lib.root_bone()];
                stats.push_str(&format!("  root0 t({:.0},{:.0},{:.0}) q({:.2},{:.2},{:.2},{:.2})", r.t[0], r.t[1], r.t[2], r.r[0], r.r[1], r.r[2], r.r[3]));
            }
        }
        if std::env::var_os("SOM_SWING").is_some() {
            if let (Some(bone), Some(a), Some(b)) =
                (skeleton.find("R_Weapon"), lib.event_s(id, "BCOL"), lib.event_s(id, "ECOL"))
            {
                if let Some(t) = lib.fastest_s(id, bone, a, b) {
                    let hit = lib.event_s(id, "APPLYDAMAGE").unwrap_or(-1.0);
                    stats.push_str(&format!("  swing peak {t:.2}s  BCOL {a:.2} APPLY {hit:.2} ECOL {b:.2}"));
                }
            }
        }
        println!("{:<48}{:>6} ms  [{}]{stats}", clip.name, clip.duration_ms(), clip.codecs.join(" "));
        if std::env::var_os("SOM_SEGS").is_some() {
            println!("      segments {:?}", clip.segment_ranges());
        }
        if std::env::var_os("SOM_EVENTS").is_some() {
            for event in &clip.events {
                println!("      {:>5} ms  {}", event.time_ms, event.text);
            }
        }
    }
    Ok(())
}

/// `som trans <bvr> <state> [depth]`: a behaviour state's transitions with the predicate trees that gate them.
fn trans(vfs: &mut Vfs, path: &str, state: &str, depth: usize) -> Result<()> {
    use som_formats::gamedb::{Entry, GameDb, Value};
    use som_formats::hash::name_hash;
    let db = GameDb::parse(vfs.read(path)?)?;
    let mut names: std::collections::HashMap<u32, String> =
        db.strings().into_iter().map(|s| (name_hash(&s), s)).collect();
    if let Ok(game_dir) = Vfs::locate() {
        if let Ok(exe) = std::fs::read(game_dir.join("x64").join("ShadowOfMordor.exe")) {
            let mut start = None;
            for (i, &b) in exe.iter().enumerate().chain(std::iter::once((exe.len(), &0u8))) {
                let ok = b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'/';
                match (ok, start) {
                    (true, None) => start = Some(i),
                    (false, Some(s0)) => {
                        if (3..=60).contains(&(i - s0)) && !exe[s0].is_ascii_digit() {
                            let text = String::from_utf8_lossy(&exe[s0..i]).into_owned();
                            names.entry(name_hash(&text)).or_insert(text);
                        }
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
    let mut by_hash: std::collections::HashMap<u32, (usize, usize)> = std::collections::HashMap::new();
    for t in 0..db.type_count() {
        let (_, f, r) = db.type_info(t).unwrap();
        for i in f..f + r {
            if let Some(n) = db.item(t, i).and_then(|e| e.name()) {
                by_hash.entry(name_hash(&n)).or_insert((t, i));
            }
        }
    }
    let resolve = |v: u32| -> Option<Entry<'_>> { db.follow(v).filter(|e| e.name().is_some()).or_else(|| by_hash.get(&v).and_then(|&(t, i)| db.item(t, i))) };
    let type_of = |v: u32| -> String {
        let t = if db.follow(v).is_some_and(|e| e.name().is_some()) { (v >> 20) as usize } else { by_hash.get(&v).map_or(usize::MAX, |p| p.0) };
        db.type_info(t).map(|t| t.0).unwrap_or_default()
    };
    let label = |h: u32| names.get(&h).cloned().unwrap_or_else(|| format!("{h:08x}"));
    fn dump<'a>(
        e: Entry<'a>,
        indent: usize,
        depth: usize,
        label: &dyn Fn(u32) -> String,
        resolve: &dyn Fn(u32) -> Option<Entry<'a>>,
        type_of: &dyn Fn(u32) -> String,
        db: &GameDb,
    ) {
        let pad = " ".repeat(indent);
        for c in e.cells() {
            let name = label(c.hash);
            if matches!(name.as_str(), "watch" | "debug" | "CharacterMinLOD" | "LowLODDelay" | "MedLODDelay" | "HighLODDelay" | "ForceEvaluateAll") {
                continue;
            }
            if c.kind == 7 {
                let pairs: Vec<(f32, f32)> = e.raw_field(c.hash).unwrap_or(&[]).iter().filter_map(|&o| Some((db.pool_f32(o)?, db.pool_f32(o + 4)?))).collect();
                println!("{pad}{name} = range {pairs:?}");
                continue;
            }
            match e.value(c) {
                Value::Strings(s) => println!("{pad}{name} = {s:?}"),
                Value::Refs(r) => println!("{pad}{name} -> {}", r.iter().map(|&x| format!("{}:{}", db.type_info((x >> 20) as usize).map(|t| t.0).unwrap_or_default(), db.follow(x).and_then(|e| e.name()).unwrap_or_default())).collect::<Vec<_>>().join(", ")),
                Value::Raw { kind: 10, values } => {
                    if values.iter().all(|&v| v == 0xffff_ffff) {
                        continue;
                    }
                    println!("{pad}{name} #");
                    for v in values {
                        match resolve(v) {
                            Some(t) => {
                                println!("{pad}  {}:{}", type_of(v), t.name().unwrap_or_default());
                                if depth > 0 && name != "node" {
                                    dump(t, indent + 4, depth - 1, label, resolve, type_of, db);
                                }
                            }
                            None if v != 0xffff_ffff => println!("{pad}  {v:#x}"),
                            None => {}
                        }
                    }
                }
                Value::Raw { kind: 2, values } => println!("{pad}{name} = {:?}", values.iter().map(|&v| f32::from_bits(v)).collect::<Vec<_>>()),
                Value::Raw { kind, values } => println!("{pad}{name} k{kind} {values:?}"),
            }
        }
    }
    let ty = (0..db.type_count()).find(|&i| db.type_info(i).is_some_and(|t| t.0 == "BlendState")).context("no BlendState")?;
    let (_, f, r) = db.type_info(ty).unwrap();
    let mut found = false;
    for i in f..f + r {
        let Some(e) = db.item(ty, i) else { continue };
        if e.name().as_deref() != Some(state) {
            continue;
        }
        found = true;
        println!("== {state}");
        for &link in e.field(name_hash("Links")).map(|v| match v { Value::Refs(r) => r, Value::Raw { values, .. } => values, _ => Vec::new() }).unwrap_or_default().iter() {
            let Some(t) = resolve(link) else { continue };
            println!("-- {}", t.name().unwrap_or_default());
            dump(t, 3, depth, &label, &resolve, &type_of, &db);
        }
    }
    anyhow::ensure!(found, "no state {state}");
    Ok(())
}

fn db(vfs: &mut Vfs, path: &str, ty: Option<&String>, item: Option<&String>) -> Result<()> {
    use som_formats::gamedb::{GameDb, Value};
    let db = GameDb::parse(vfs.read(path)?)?;
    let mut names: std::collections::HashMap<u32, String> =
        db.strings().into_iter().map(|s| (som_formats::hash::name_hash(&s), s)).collect();
    // Field names are hashes in the database; the exe still carries them as reflection strings.
    if let Ok(game_dir) = Vfs::locate() {
        if let Ok(exe) = std::fs::read(game_dir.join("x64").join("ShadowOfMordor.exe")) {
            let mut start = None;
            for (i, &b) in exe.iter().enumerate().chain(std::iter::once((exe.len(), &0u8))) {
                let ok = b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'/';
                match (ok, start) {
                    (true, None) => start = Some(i),
                    (false, Some(s0)) => {
                        if (3..=60).contains(&(i - s0)) && !exe[s0].is_ascii_digit() {
                            let text = String::from_utf8_lossy(&exe[s0..i]).into_owned();
                            names.entry(som_formats::hash::name_hash(&text)).or_insert(text);
                        }
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
    // Records are also referenced by the hash of their name (kind 10).
    let mut by_hash: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for t in 0..db.type_count() {
        let (tn, f, r) = db.type_info(t).unwrap();
        for i in f..f + r {
            if let Some(n) = db.item(t, i).and_then(|e| e.name()) {
                by_hash.entry(som_formats::hash::name_hash(&n)).or_insert_with(|| format!("{tn}:{n}"));
            }
        }
    }
    let label = |h: u32| names.get(&h).cloned().unwrap_or_else(|| format!("{h:08x}"));
    let find = |name: &str| (0..db.type_count()).find(|&i| db.type_info(i).is_some_and(|t| t.0.eq_ignore_ascii_case(name)));
    let show = |index: usize, i: usize| {
        let Some(e) = db.item(index, i) else { return };
        println!("[{index}:{i}] {}  raw {:?}", e.name().unwrap_or_default(), e.raw_values());
        for c in e.cells() {
            if c.kind == 7 {
                let pairs: Vec<(f32, f32)> = e.raw_field(c.hash).unwrap_or(&[]).iter().filter_map(|&o| Some((db.pool_f32(o)?, db.pool_f32(o + 4)?))).collect();
                println!("    {} (kind 7) = range {pairs:?}", label(c.hash));
                continue;
            }
            let v = match e.value(c) {
                // Kind 8 is a vector: an offset into the float pool (three floats), not a string.
                Value::Strings(_) if c.kind == 8 => e
                    .raw_field(c.hash)
                    .unwrap_or(&[])
                    .iter()
                    .map(|&o| format!("{:?}", [0, 4, 8].map(|k| db.pool_f32(o + k).unwrap_or(f32::NAN))))
                    .collect::<Vec<_>>()
                    .join(", "),
                Value::Strings(s) => format!("{s:?}"),
                Value::Refs(r) => format!(
                    "-> {}",
                    r.iter()
                        .map(|&x| {
                            let t = db.type_info((x >> 20) as usize).map(|t| t.0).unwrap_or_default();
                            let n = db.follow(x).and_then(|e| e.name()).unwrap_or_default();
                            format!("{t}:{}{}", x & 0xf_ffff, if n.is_empty() { String::new() } else { format!("({n})") })
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Value::Raw { kind: 10, values } => format!(
                    "# {}",
                    values
                        .iter()
                        .map(|v| {
                            db.follow(*v)
                                .map(|e| format!("{}:{}", db.type_info((*v >> 20) as usize).map(|t| t.0).unwrap_or_default(), e.name().unwrap_or_default()))
                                .or_else(|| by_hash.get(v).cloned())
                                .unwrap_or_else(|| format!("{v:#x}"))
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                // Booleans (kind 1) are bits of the record's first value word; the cell's `first` is the bit.
                Value::Raw { kind: 1, .. } => {
                    let word = e.raw_values().first().copied().unwrap_or(0);
                    format!("bit {} of {word:#x} = {}", c.first, (word >> c.first) & 1 == 1)
                }
                Value::Raw { kind, values } => format!("k{kind} {values:?}"),
            };
            println!("    {} (kind {}) = {v}", label(c.hash), c.kind);
        }
    };
    match (ty, item) {
        (None, _) => {
            for i in 0..db.type_count() {
                let (n, f, r) = db.type_info(i).unwrap();
                println!("{i:3} {n}  ({f} structs, {r} records)");
            }
        }
        (Some(t), None) => {
            let index = t.parse().ok().or_else(|| find(t)).context("no such type")?;
            let (n, f, r) = db.type_info(index).context("type")?;
            println!("{n}: {f} structs, {r} records");
            for i in 0..f + r {
                let e = db.item(index, i).unwrap();
                println!("  {i}: {}", e.name().unwrap_or_default());
            }
        }
        (Some(t), Some(i)) => {
            let index = t.parse().ok().or_else(|| find(t)).context("no such type")?;
            show(index, i.parse()?);
        }
    }
    Ok(())
}
