use std::{fs::File, path::Path};

use anyhow::anyhow;

use crate::fs::get_cache_dir;

struct Ipsw {
    pub url: String,
    pub name: String,
    pub version: String,
    pub release_date: String,
}

pub struct IpswRegistry {
    items: Vec<Ipsw>,
}

impl IpswRegistry {
    fn load() -> anyhow::Result<Vec<Ipsw>> {
        let items = vec![Ipsw {
            url: "https://updates.cdn-apple.com/2024FallFCS/fullrestores/072-01423/566E5B4E-1100-4643-91B3-131247351844/UniversalMac_15.0.1_24A348_Restore.ipsw".to_string(),
            name: "UniversalMac_15.0.1_24A348_Restore.ipsw".to_string(),
            version:  "15.0.1".to_string(),
            release_date: "04/10/2024".to_string()
        }, Ipsw {
            url: "https://updates.cdn-apple.com/2024FallFCS/fullrestores/062-78489/BDA44327-C79E-4608-A7E0-455A7E91911F/UniversalMac_15.0_24A335_Restore.ipsw".to_string(),
            name: "UniversalMac_15.0_24A335_Restore.ipsw".to_string(),
            version: "15.0.0".to_string(),
            release_date: "16/09/2024".to_string()
        }, Ipsw {
            url: "https://updates.cdn-apple.com/2024SummerFCS/fullrestores/062-52859/932E0A8F-6644-4759-82DA-F8FA8DEA806A/UniversalMac_14.6.1_23G93_Restore.ipsw".to_string(),
            name: "UniversalMac_14.6.1_23G93_Restore.ipsw".to_string(),
            version: "14.6.1".to_string(),
            release_date: "7/08/2024".to_string()
        },];

        Ok(items)
    }

    pub fn new() -> Self {
        let items = Self::load().unwrap();
        IpswRegistry { items }
    }

    fn select(&self, version: &str) -> Option<&Ipsw> {
        self.items.iter().find(|i| i.version == version)
    }

    pub fn download(&self, version: &str) -> anyhow::Result<String> {
        let chosen = self.select(version);

        if chosen.is_none() {
            return Err(anyhow!("Version {} not in registry", version));
        }

        let version = chosen.unwrap();

        let cache_dir = get_cache_dir().unwrap();
        let ipsw_path = Path::new(&cache_dir)
            .join("codes.nvd.BentoBox")
            .join(&version.name);

        // WARNING: this is raceable, which could result in the download happening twice.
        // I don't really care for now. If the race happens the loser will fail anyways
        // because later I just blindly create the file. This is good enough for now.
        if ipsw_path.exists() {
            return Ok(ipsw_path.to_str().unwrap().to_string());
        }

        println!("Starting to download the restore file");

        // Download the file
        let mut response = reqwest::blocking::get(&version.url)?;

        // let size = response
        //     .headers()
        //     .get("Content-Length")
        //     .and_then(|len| len.to_str().ok())
        //     .and_then(|s| s.parse().ok())
        //     .unwrap_or(0);

        let mut file =
            File::create(ipsw_path.to_str().unwrap()).expect("should not fail creating this file");

        // let mut downloaded: u64 = 0;
        // let mut stream = response.bytes_stream();

        // while let Some(chunk) = stream.next() {
        //     let chunk = chunk?;
        //     file.write_all(&chunk)?;
        //     downloaded += chunk.len() as u64;
        //
        //     // Update the progress bar
        // }
        //

        response.copy_to(&mut file)?;

        println!("Download finished");

        Ok(ipsw_path.to_str().unwrap().to_string())
    }
}
