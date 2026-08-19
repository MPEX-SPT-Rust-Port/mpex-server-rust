//! Build-time generator for `SPT_Data/checks.dat`, invoked by the `PreBuildHashFile` target in
//! `SPTarkov.Server.Assets.csproj` on Release builds. Replaces the former `build/PostBuild.cs`
//! dotnet file-based app so the build no longer pulls `System.IO.Hashing` from NuGet.

use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let Some(spt_data) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: gen_checks <SPT_Data dir>");
        std::process::exit(2);
    };

    let generated = match spt_native::verify::generate(spt_data.clone()).await {
        Ok(generated) => generated,
        Err(e) => {
            eprintln!("gen_checks: {e}");
            std::process::exit(1);
        }
    };

    let out = spt_data.join("checks.dat");
    if let Err(e) = std::fs::write(&out, &generated.base64) {
        eprintln!("gen_checks: cannot write {}: {e}", out.display());
        std::process::exit(1);
    }

    println!("Hashed {} files", generated.hashed);
}
