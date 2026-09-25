// Flash-test generator: from-scratch LID20001 with N cities (shared lid_format::city writer).
// usage: gen_cities [out.DAT] [cities.tsv]   (TSV lines: NAME<TAB>lat<TAB>lon; default = 10 PL cities)
use lid_format::city::{build_city_file, selfcheck, CityEntry};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stock_path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                      CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT";
    let out_path = args
        .get(1)
        .cloned()
        .unwrap_or("/tmp/opencode/zzcity10.DAT".into());
    let stock = std::fs::read(stock_path).expect("read stock");
    let cities: Vec<CityEntry> = match args.get(2) {
        Some(tsv) => std::fs::read_to_string(tsv)
            .expect("read tsv")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let mut it = l.split('\t');
                let (n, la, lo) = (
                    it.next().expect("name"),
                    it.next().expect("lat"),
                    it.next().expect("lon"),
                );
                CityEntry::from_deg(n, lo.parse().expect("lon"), la.parse().expect("lat"))
            })
            .collect(),
        None => [
            ("BYDGOSZCZ", 18.0084, 53.1235),
            ("GDAŃSK", 18.6464, 54.3520),
            ("KRAKÓW", 19.9450, 50.0647),
            ("LUBLIN", 22.5684, 51.2465),
            ("POZNAŃ", 16.9252, 52.4064),
            ("RADOM", 21.1471, 51.4027),
            ("SZCZECIN", 14.5528, 53.4285),
            ("WARSZAWA", 21.0118, 52.2298),
            ("WROCŁAW", 17.0385, 51.1079),
            ("ŁÓDŹ", 19.4560, 51.7592),
        ]
        .iter()
        .map(|(n, lo, la)| CityEntry::from_deg(n, *lo, *la))
        .collect(),
    };
    let mut sorted: Vec<&CityEntry> = cities.iter().collect();
    sorted.sort_unstable_by_key(|c| c.name.as_bytes());
    println!(
        "expected list order: {:?}",
        sorted.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
    );
    let data = build_city_file(&cities, &stock);
    std::fs::write(&out_path, &data).expect("write");
    println!("wrote {} bytes to {}", data.len(), out_path);
    selfcheck(&data, &stock, &cities).expect("selfcheck");
    println!("OK: {} cities verified", cities.len());
}
