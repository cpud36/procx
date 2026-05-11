use procx::{arg, args, cmd};

fn main() {
    let lto = "off";

    let output_name = "hello";
    let output_ext = ".exe";
    let output_flags = args!("-o={output_name}{output_ext}");
    let lto_flag = arg!("-flto={lto}");
    let c = cmd!("cc {lto_flag} {output_flags..} hello.c");
    eprintln!("running {c}");
}
