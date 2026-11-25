fn main() {
    let mut m = std::collections::BTreeMap::<String, (u32, u32)>::new();
    for f in std::env::args().skip(1) {
        let b = std::fs::read(&f).unwrap();
        let Ok(c) = victory_shader::Container::parse(&b) else { continue };
        let Some(d) = c.part(victory_shader::FourCC::DXIL) else { continue };
        let Ok(ps) = victory_shader::native_programs(d.data) else { continue };
        let mut seen = std::collections::BTreeSet::new();
        for n in &ps {
            for i in victory_shader::gfx10::decode(&n.code).unwrap_or_default() {
                let t = i.text(n.wave64);
                if t.starts_with("image_") {
                    let mut k = t.split(' ').next().unwrap().to_owned();
                    for w in ["tfe", "lwe"] { if t.contains(&format!(" {w}")) { k += " "; k += w; } }
                    m.entry(k.clone()).or_default().0 += 1;
                    if seen.insert(k.clone()) { m.get_mut(&k).unwrap().1 += 1; }
                }
            }
        }
    }
    for (k, v) in m { println!("{k}: {} ({} programmes)", v.0, v.1); }
}
