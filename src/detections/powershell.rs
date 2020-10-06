use crate::detections::utils;
use crate::models::event;
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
extern crate csv;

pub struct PowerShell {}

impl PowerShell {
    pub fn new() -> PowerShell {
        PowerShell {}
    }

    pub fn detection(
        &mut self,
        event_id: String,
        _system: &event::System,
        event_data: HashMap<String, String>,
    ) {
        if event_id == "4103" {
            &self.execute_pipeline(&event_data);
        } else if event_id == "4104" {
            &self.execute_remote_command(&event_data);
        }
    }

    fn execute_pipeline(&mut self, event_data: &HashMap<String, String>) {
        // パイプライン実行をしています
        let default = String::from("");
        let commandline = event_data.get("ContextInfo").unwrap_or(&default);

        if commandline.contains("Host Application")
            || commandline.contains("ホスト アプリケーション")
        {
            let rm_before =
                Regex::new("(?ms)^.*(ホスト アプリケーション|Host Application) = ").unwrap();
            let rm_after = Regex::new("(?ms)\n.*$").unwrap();

            let temp = rm_before.replace_all(commandline, "");
            let command = rm_after.replace_all(&temp, "");

            let mut f = File::open("whitelist.txt").expect("file not found");
            let mut contents = String::new();
            let _ = f.read_to_string(&mut contents);

            let rdr = csv::Reader::from_reader(contents.as_bytes());
            if command != "" {
                utils::check_command(4103, &command, 1000, 0, &default, &default, rdr);
            }
        }
        return;
    }

    fn execute_remote_command(&mut self, event_data: &HashMap<String, String>) {
        // リモートコマンドを実行します
        let default = String::from("");
        let message_num = event_data.get("MessageNumber");
        let commandline = event_data.get("ScriptBlockText").unwrap_or(&default);

        let mut f = File::open("whitelist.txt").expect("file not found");
        let mut contents = String::new();
        let _ = f.read_to_string(&mut contents);

        let rdr = csv::Reader::from_reader(contents.as_bytes());
        match message_num {
            Some(_) => utils::check_command(4104, &commandline, 1000, 0, &default, &default, rdr),
            _ => {}
        }

        return;
    }
}
