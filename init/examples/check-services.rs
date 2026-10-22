// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Host-side admission check using init's production parser and policy.
use hyper_bootstrap_stage1_init::{
    bootstrap_policy::BootstrapPolicy, manifest, profile::Profile, supervision,
};
use std::{env, fs, process::ExitCode};

fn check() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let first = args
        .next()
        .ok_or("usage: check-services [--profile PROFILE] MANIFEST [ARCHIVE_PATH ...]")?;
    let (profile, path) = if first == "--profile" {
        let path = args.next().ok_or("--profile requires a profile path")?;
        let text = fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
        (
            Some(Profile::parse(&text).map_err(|error| format!("{path}: {error}"))?),
            args.next().ok_or("missing service manifest path")?,
        )
    } else {
        (None, first)
    };
    let images: Vec<String> = args.collect();
    let text = fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
    let manifest = manifest::parse(&text).map_err(|error| format!("{path}: {error:?}"))?;
    if let Some(profile) = &profile {
        profile
            .validate(&manifest)
            .map_err(|error| format!("{path}: {error}"))?;
    } else {
        manifest::validate(&manifest, &BootstrapPolicy)
            .map_err(|error| format!("{path}: {error:?}"))?;
    }
    supervision::validate(&manifest).map_err(|error| format!("{path}: {error:?}"))?;
    if !images.is_empty() {
        for service in manifest.services() {
            if !images.iter().any(|image| {
                image.trim_start_matches('/') == service.image().trim_start_matches('/')
            }) {
                return Err(format!(
                    "{path}: service {:?}: image {:?} is absent from the archive",
                    service.name(),
                    service.image()
                ));
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match check() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("service manifest: {error}");
            ExitCode::FAILURE
        }
    }
}
