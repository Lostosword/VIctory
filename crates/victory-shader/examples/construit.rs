//! Classe les programmes de calcul : ceux du constructeur de structures (table étendue), ceux qui testent un rayon, traduits ou non.
fn main() {
    victory_shader::emit::set_host_max_wave(32);
    for f in std::env::args().skip(1) {
        let b = std::fs::read(&f).unwrap();
        let Ok(c) = victory_shader::Container::parse(&b) else { continue };
        let Some(d) = c.part(victory_shader::FourCC::DXIL) else { continue };
        let Ok(ps) = victory_shader::native_programs(d.data) else { continue };
        let spilled = ps.iter().any(victory_shader::spill::is_spilled);
        let wave64 = ps.iter().any(|n| n.wave64);
        let name = std::path::Path::new(&f).file_stem().unwrap().to_string_lossy().into_owned();
        let bvh = ps.iter().any(|n| victory_shader::gfx10::decode(&n.code).is_ok_and(|v| v.iter().any(|i| i.name().is_some_and(|m| m.starts_with("image_bvh")))));
        if !spilled && !bvh { continue }
        let t = match victory_shader::emit::translate(&b) { Ok(t) => format!("ok {} octets", t.dxil.len()), Err(e) => format!("NON {e}") };
        println!("{name} {} {} wave{} {t}", if spilled { "CONSTRUCTEUR" } else { "-" }, if bvh { "RAYON" } else { "-" }, if wave64 { 64 } else { 32 });
    }
}
