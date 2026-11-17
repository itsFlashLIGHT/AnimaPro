//! Small controlled process used by export lifecycle tests on every platform.
use std::{env, fs, io, thread, time::Duration};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    let executable = std::path::Path::new(&args[0]);
    let mode = executable.file_stem().unwrap().to_string_lossy();
    let preflight = args.iter().any(|arg| arg == "-encoders");
    if preflight && mode == "unresponsive" {
        fs::write(executable.with_extension("started"), b"started").unwrap();
        thread::sleep(Duration::from_secs(30));
    } else if preflight && mode == "no_encoder" {
        println!(" V..... other_encoder");
    } else if preflight {
        println!(" V..... libx264 controlled encoder");
    } else {
        match mode.as_ref() {
            "blocked" => {
                fs::write(executable.with_extension("started"), b"started").unwrap();
                thread::sleep(Duration::from_secs(30));
            }
            "noisy_failure" => {
                for _ in 0..3000 {
                    eprintln!("encoder diagnostic message");
                }
                std::process::exit(1);
            }
            "output" => {
                io::copy(&mut io::stdin(), &mut io::sink()).unwrap();
                fs::write(args.last().unwrap(), b"encoded").unwrap();
            }
            _ => std::process::exit(1),
        }
    }
}
