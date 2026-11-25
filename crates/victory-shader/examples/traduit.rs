//! Traduit des conteneurs de shaders enregistrés (VICTORY_SHADER_SAVE) : `traduit <sortie> <fichiers .cdxil>`.
fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().expect("dossier"));
    victory_shader::emit::set_host_max_wave(32);
    for f in args {
        let path = std::path::PathBuf::from(&f);
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| if std::env::var_os("SANS_CLIP").is_some() { { let un = std::env::var("PIXEL").ok().and_then(|p| std::fs::read(p).ok()).map(|p| victory_shader::emit::unread_inputs(&p)).unwrap_or_default(); victory_shader::emit::translate_slim(&b, &un) } } else { victory_shader::emit::translate(&b) }.map_err(|e| e.to_string())) {
            Ok(t) => std::fs::write(out.join(format!("{name}.dxil")), &t.dxil).unwrap(),
            Err(e) => println!("{name}: {e}"),
        }
    }
}
