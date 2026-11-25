fn main() {
    for f in std::env::args().skip(1) {
        let b = std::fs::read(&f).unwrap();
        let c = victory_shader::Container::parse(&b).unwrap();
        let d = c.part(victory_shader::FourCC::DXIL).unwrap();
        for n in victory_shader::native_programs(d.data).unwrap() {
            let h = n.code.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3));
            println!("{f} {:?} {:#018x} {}", n.stage, h, n.code.len());
        }
    }
}
