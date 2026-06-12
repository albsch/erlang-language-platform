/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::str::FromStr;

use time::OffsetDateTime;
use time::format_description;

const CI: &str = "CI";
const SOURCE_DATE_EPOCH: &str = "SOURCE_DATE_EPOCH";
const EQWALIZER_DIR: &str = "EQWALIZER_DIR";
const EQWALIZER_SUPPORT_DIR: &str = "EQWALIZER_SUPPORT_DIR";
const CARGO_MANIFEST_DIR: &str = "CARGO_MANIFEST_DIR";
const ELP_ETYLIZER_ESCRIPT: &str = "ELP_ETYLIZER_ESCRIPT";
const ELP_ETYLIZER_ESPRESSO: &str = "ELP_ETYLIZER_ESPRESSO";
const ELP_ETYLIZER_VERSION: &str = "ELP_ETYLIZER_VERSION";

fn main() {
    let date_format =
        format_description::parse("build-[year]-[month]-[day]").expect("wrong format");

    let is_ci = env::var(CI).is_ok();
    let epoch = env::var(SOURCE_DATE_EPOCH);
    let build_id = if is_ci || epoch.is_ok() {
        let date = match epoch {
            Ok(v) => {
                let timestamp = i64::from_str(&v).expect("parsing SOURCE_DATE_EPOCH");
                OffsetDateTime::from_unix_timestamp(timestamp).expect("parsing SOURCE_DATE_EPOCH")
            }
            Err(std::env::VarError::NotPresent) => OffsetDateTime::now_utc(),
            Err(e) => panic!("Error getting SOURCE_DATE_EPOCH: {e}"),
        };
        date.format(&date_format).expect("formatting date")
    } else {
        "local".to_string()
    };
    let eqwalizer_dir = env::var(EQWALIZER_DIR);
    let cargo_manifest_dir = env::var(CARGO_MANIFEST_DIR)
        .expect("CARGO_MANIFEST_DIR should be set automatically by cargo");
    let eqwalizer_support_dir = match eqwalizer_dir {
        Ok(eqwalizer_support_dir) => format!("{eqwalizer_support_dir}/../eqwalizer_support"),
        Err(_) => format!("{cargo_manifest_dir}/../../../eqwalizer/eqwalizer_support"),
    };

    // Optionally embed the etylizer escript into the binary (like the erlang_service escript).
    // When ELP_ETYLIZER_ESCRIPT points at a prebuilt escript (the CI builds it from the etylizer
    // repo), copy it next to the binary so etylizer.rs can `include_bytes!` it. Otherwise write an
    // empty placeholder, and the binary falls back to ELP_ETYLIZER_PATH / `etylizer` on PATH.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let etylizer_dest = std::path::Path::new(&out_dir).join("etylizer_escript");
    match env::var_os(ELP_ETYLIZER_ESCRIPT) {
        Some(src) => {
            let src = std::path::PathBuf::from(src);
            std::fs::copy(&src, &etylizer_dest).expect("copying ELP_ETYLIZER_ESCRIPT failed");
            println!("cargo:rerun-if-changed={}", src.display());
        }
        None => {
            std::fs::write(&etylizer_dest, b"").expect("writing empty etylizer placeholder failed");
        }
    }

    // etylizer shells out to a native `espresso` binary (the Berkeley logic minimizer) at
    // runtime. Embed the per-OS/arch espresso the CI built next to the escript so etylizer.rs can
    // `include_bytes!` it, extract it, and point etylizer at it via the ETYLIZER_ESPRESSO env var.
    // Without this the binary depends on a prior native etylizer build having populated
    // ~/.cache/etylizer/espresso. Empty placeholder when not bundled.
    let espresso_dest = std::path::Path::new(&out_dir).join("etylizer_espresso");
    match env::var_os(ELP_ETYLIZER_ESPRESSO) {
        Some(src) => {
            let src = std::path::PathBuf::from(src);
            std::fs::copy(&src, &espresso_dest).expect("copying ELP_ETYLIZER_ESPRESSO failed");
            println!("cargo:rerun-if-changed={}", src.display());
        }
        None => {
            std::fs::write(&espresso_dest, b"").expect("writing empty espresso placeholder failed");
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={SOURCE_DATE_EPOCH}");
    println!("cargo:rerun-if-env-changed={CI}");
    println!("cargo:rustc-env=BUILD_ID={build_id}");
    println!("cargo:rustc-env={EQWALIZER_SUPPORT_DIR}={eqwalizer_support_dir}");
    println!("cargo:rerun-if-env-changed={EQWALIZER_DIR}");
    println!(
        "cargo:rustc-env=ELP_ETYLIZER_ESCRIPT_PATH={}",
        etylizer_dest.display()
    );
    println!("cargo:rerun-if-env-changed={ELP_ETYLIZER_ESCRIPT}");
    println!(
        "cargo:rustc-env=ELP_ETYLIZER_ESPRESSO_PATH={}",
        espresso_dest.display()
    );
    println!("cargo:rerun-if-env-changed={ELP_ETYLIZER_ESPRESSO}");

    // The bundled etylizer's version (git short SHA), provided by the CI. "unknown" otherwise.
    let etylizer_version =
        env::var(ELP_ETYLIZER_VERSION).unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env={ELP_ETYLIZER_VERSION}={etylizer_version}");
    println!("cargo:rerun-if-env-changed={ELP_ETYLIZER_VERSION}");
}
