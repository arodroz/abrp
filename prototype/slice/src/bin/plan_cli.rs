// Throwaway performance prototype: pack dir + origin/dest -> one plan.
// Prints the response JSON on stdout; prints pack-load time and peak RSS on stderr.

use std::mem::MaybeUninit;
use std::time::Instant;

use planner::Planner;
use serde_json::json;

struct Args {
    pack: String,
    origin: (f64, f64),
    dest: (f64, f64),
    depart_soc: Option<f64>,
    arrival_min_soc: Option<f64>,
    charger_arrival_min_soc: Option<f64>,
    charger_max_soc: Option<f64>,
    stops_bias: Option<f64>,
}

fn parse_latlon(s: &str) -> (f64, f64) {
    let mut it = s.split(',');
    let lat: f64 = it.next().unwrap().trim().parse().unwrap();
    let lon: f64 = it.next().unwrap().trim().parse().unwrap();
    (lat, lon)
}

fn parse_args() -> Args {
    let mut pack = None;
    let mut origin = None;
    let mut dest = None;
    let mut depart_soc = None;
    let mut arrival_min_soc = None;
    let mut charger_arrival_min_soc = None;
    let mut charger_max_soc = None;
    let mut stops_bias = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pack" => pack = it.next(),
            "--origin" => origin = it.next().map(|s| parse_latlon(&s)),
            "--dest" => dest = it.next().map(|s| parse_latlon(&s)),
            "--depart-soc" => depart_soc = it.next().and_then(|s| s.parse().ok()),
            "--arrival-min-soc" => arrival_min_soc = it.next().and_then(|s| s.parse().ok()),
            "--charger-arrival-min-soc" => {
                charger_arrival_min_soc = it.next().and_then(|s| s.parse().ok())
            }
            "--charger-max-soc" => charger_max_soc = it.next().and_then(|s| s.parse().ok()),
            "--stops-bias" => stops_bias = it.next().and_then(|s| s.parse().ok()),
            _ => {}
        }
    }
    Args {
        pack: pack.expect("--pack <dir> required"),
        origin: origin.expect("--origin lat,lon required"),
        dest: dest.expect("--dest lat,lon required"),
        depart_soc,
        arrival_min_soc,
        charger_arrival_min_soc,
        charger_max_soc,
        stops_bias,
    }
}

fn peak_rss_mb() -> f64 {
    unsafe {
        let mut usage = MaybeUninit::<libc::rusage>::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr());
        let usage = usage.assume_init();
        // macOS reports ru_maxrss in bytes; Linux reports it in KB.
        #[cfg(target_os = "macos")]
        let bytes = usage.ru_maxrss as f64;
        #[cfg(not(target_os = "macos"))]
        let bytes = usage.ru_maxrss as f64 * 1024.0;
        bytes / (1024.0 * 1024.0)
    }
}

fn main() {
    let args = parse_args();

    let load_start = Instant::now();
    let planner = Planner::new(args.pack);
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[plan_cli] pack load: {:.2} ms", load_ms);

    let mut req = json!({
        "origin": [args.origin.0, args.origin.1],
        "dest": [args.dest.0, args.dest.1],
    });
    if let Some(v) = args.depart_soc {
        req["depart_soc"] = json!(v);
    }
    if let Some(v) = args.arrival_min_soc {
        req["arrival_min_soc"] = json!(v);
    }
    if let Some(v) = args.charger_arrival_min_soc {
        req["charger_arrival_min_soc"] = json!(v);
    }
    if let Some(v) = args.charger_max_soc {
        req["charger_max_soc"] = json!(v);
    }
    if let Some(v) = args.stops_bias {
        req["stops_bias"] = json!(v);
    }

    let response_json = planner.plan_json(req.to_string());
    println!("{response_json}");

    eprintln!("[plan_cli] peak RSS: {:.2} MB", peak_rss_mb());
}
