fn main() {
    let mut a = std::env::args().skip(1);
    let b = std::fs::read(a.next().unwrap()).unwrap();
    let (from, to) = (a.next().map_or(0, |s| usize::from_str_radix(&s, 16).unwrap()), a.next().map_or(usize::MAX, |s| usize::from_str_radix(&s, 16).unwrap()));
    let c = victory_shader::Container::parse(&b).unwrap();
    let d = c.part(victory_shader::FourCC::DXIL).unwrap();
    for n in victory_shader::native_programs(d.data).unwrap() {
        println!("; {:?} wave64 {} layout {:?}", n.stage, n.wave64, n.layout);
        for i in victory_shader::gfx10::decode(&n.code).unwrap() {
            let at = i.offset as usize;
            if at >= from && at <= to { println!("{at:x}: {}", i.text(n.wave64)); }
        }
    }
}
