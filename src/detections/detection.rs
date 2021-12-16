extern crate csv;

use crate::detections::rule::AggResult;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Instant;
use tokio::{runtime::Runtime, spawn, task::JoinHandle};

use crate::detections::configs;
use crate::detections::print::AlertMessage;
use crate::detections::print::MESSAGES;
use crate::detections::rule;
use crate::detections::rule::RuleNode;
use crate::detections::utils::get_serde_number_to_string;
use crate::filter;
use crate::yaml::ParseYaml;

use std::sync::Arc;

const DIRPATH_RULES: &str = "rules";

// イベントファイルの1レコード分の情報を保持する構造体
#[derive(Clone, Debug)]
pub struct EvtxRecordInfo {
    pub evtx_filepath: String, // イベントファイルのファイルパス　ログで出力するときに使う
    pub record: Value,         // 1レコード分のデータをJSON形式にシリアライズしたもの
    pub data_string: String,
}

impl EvtxRecordInfo {
    pub fn new(evtx_filepath: String, record: Value, data_string: String) -> EvtxRecordInfo {
        return EvtxRecordInfo {
            evtx_filepath: evtx_filepath,
            record: record,
            data_string: data_string,
        };
    }
}

#[derive(Debug)]
pub struct Detection {
    pub rules: Vec<RuleNode>,
}

impl Detection {
    pub fn new(rules: Vec<RuleNode>) -> Detection {
        return Detection { rules: rules };
    }

    pub fn start(self, rt: &Runtime, records: Vec<EvtxRecordInfo>) -> Self {
        return rt.block_on(self.execute_rules(records));
    }

    // ルールファイルをパースします。
    pub fn parse_rule_files(
        level: String,
        rulespath: Option<&str>,
        exclude_ids: &filter::RuleExclude,
    ) -> Vec<RuleNode> {
        // ルールファイルのパースを実行
        let mut rulefile_loader = ParseYaml::new();
        let result_readdir =
            rulefile_loader.read_dir(rulespath.unwrap_or(DIRPATH_RULES), &level, exclude_ids);
        if result_readdir.is_err() {
            AlertMessage::alert(
                &mut std::io::stderr().lock(),
                format!("{}", result_readdir.unwrap_err()),
            )
            .ok();
            return vec![];
        }
        let mut parseerror_count = rulefile_loader.errorrule_count;
        let return_if_success = |mut rule: RuleNode| {
            let err_msgs_result = rule.init();
            if err_msgs_result.is_ok() {
                return Option::Some(rule);
            }

            // ruleファイルのパースに失敗した場合はエラー出力
            err_msgs_result.err().iter().for_each(|err_msgs| {
                let errmsg_body =
                    format!("Failed to parse rule file. (FilePath : {})", rule.rulepath);
                AlertMessage::warn(&mut std::io::stdout().lock(), errmsg_body).ok();

                err_msgs.iter().for_each(|err_msg| {
                    AlertMessage::warn(&mut std::io::stdout().lock(), err_msg.to_string()).ok();
                });
                parseerror_count += 1;
                println!(""); // 一行開けるためのprintln
            });
            return Option::None;
        };
        // parse rule files
        let ret = rulefile_loader
            .files
            .into_iter()
            .map(|rule_file_tuple| rule::create_rule(rule_file_tuple.0, rule_file_tuple.1))
            .filter_map(return_if_success)
            .collect();
        Detection::print_rule_load_info(
            &rulefile_loader.rulecounter,
            &parseerror_count,
            &rulefile_loader.ignorerule_count,
        );
        return ret;
    }

    // 複数のイベントレコードに対して、複数のルールを1個実行します。
    async fn execute_rules(mut self, records: Vec<EvtxRecordInfo>) -> Self {
        let records_arc = Arc::new(records);
        // // 各rule毎にスレッドを作成して、スレッドを起動する。
        let rules = self.rules;
        let handles: Vec<JoinHandle<RuleNode>> = rules
            .into_iter()
            .map(|rule| {
                let records_cloned = Arc::clone(&records_arc);
                return spawn(async move {
                    let moved_rule = Detection::execute_rule(rule, records_cloned);
                    return moved_rule;
                });
            })
            .collect();

        // 全スレッドの実行完了を待機
        let mut rules = vec![];
        for handle in handles {
            let ret_rule = handle.await.unwrap();
            rules.push(ret_rule);
        }

        // この関数の先頭でrules.into_iter()を呼び出している。それにより所有権がmapのruleを経由し、execute_ruleの引数に渡しているruleに移っているので、self.rulesには所有権が無くなっている。
        // 所有権を失ったメンバー変数を持つオブジェクトをreturnするコードを書くと、コンパイラが怒になるので(E0382という番号のコンパイルエラー)、ここでself.rulesに所有権を戻している。
        // self.rulesが再度所有権を取り戻せるように、Detection::execute_ruleで引数に渡したruleを戻り値として返すようにしている。
        self.rules = rules;

        return self;
    }

    pub fn add_aggcondtion_msg(&self) {
        for rule in &self.rules {
            if !rule.has_agg_condition() {
                continue;
            }

            let agg_results = rule.judge_satisfy_aggcondition();
            for value in agg_results {
                Detection::insert_agg_message(rule, value);
            }
        }
    }

    pub fn print_unique_results(&self) {
        let rules = &self.rules;
        let levellabel = Vec::from([
            "Critical",
            "High",
            "Medium",
            "Low",
            "Informational",
            "Undefined",
        ]);
        // levclcounts is [(Undefined), (Informational), (Low),(Medium),(High),(Critical)]
        let mut levelcounts = Vec::from([0, 0, 0, 0, 0, 0]);
        for rule in rules.into_iter() {
            if rule.check_exist_countdata() {
                let suffix = configs::LEVELMAP
                    .get(
                        &rule.yaml["level"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned()
                            .to_uppercase(),
                    )
                    .unwrap_or(&0);
                levelcounts[*suffix as usize] += 1;
            }
        }
        let mut total_unique = 0;
        levelcounts.reverse();
        for (i, value) in levelcounts.iter().enumerate() {
            println!("{} alerts: {}", levellabel[i], value);
            total_unique += value;
        }
        println!("Unique alerts detected: {}", total_unique);
    }

    // 複数のイベントレコードに対して、ルールを1個実行します。
    fn execute_rule(mut rule: RuleNode, records: Arc<Vec<EvtxRecordInfo>>) -> RuleNode {
        let start = Instant::now();
        let records = &*records;
        let agg_condition = rule.has_agg_condition();
        for record_info in records {
            let result = rule.select(&record_info.evtx_filepath, &record_info);
            if !result {
                continue;
            }
            // aggregation conditionが存在しない場合はそのまま出力対応を行う
            if !agg_condition {
                Detection::insert_message(&rule, &record_info);
            }
        }

        rule.duration += start.elapsed();
        return rule;
    }

    /// 条件に合致したレコードを表示するための関数
    fn insert_message(rule: &RuleNode, record_info: &EvtxRecordInfo) {
        MESSAGES.lock().unwrap().insert(
            record_info.evtx_filepath.to_string(),
            rule.rulepath.to_string(),
            &record_info.record,
            rule.yaml["level"].as_str().unwrap_or("-").to_string(),
            record_info.record["Event"]["System"]["Computer"]
                .to_string()
                .replace("\"", ""),
            get_serde_number_to_string(&record_info.record["Event"]["System"]["EventID"])
                .unwrap_or("-".to_owned())
                .to_string(),
            rule.yaml["title"].as_str().unwrap_or("").to_string(),
            rule.yaml["output"].as_str().unwrap_or("").to_string(),
        );
    }

    /// insert aggregation condition detection message to output stack
    fn insert_agg_message(rule: &RuleNode, agg_result: AggResult) {
        let output = Detection::create_count_output(rule, &agg_result);
        MESSAGES.lock().unwrap().insert_message(
            agg_result.filepath,
            rule.rulepath.to_string(),
            agg_result.start_timedate,
            rule.yaml["level"].as_str().unwrap_or("").to_string(),
            "-".to_string(),
            "-".to_string(),
            rule.yaml["title"].as_str().unwrap_or("").to_string(),
            output.to_string(),
        )
    }

    ///aggregation conditionのcount部分の検知出力文の文字列を返す関数
    fn create_count_output(rule: &RuleNode, agg_result: &AggResult) -> String {
        let mut ret: String = "count(".to_owned();
        let key: Vec<&str> = agg_result.key.split("_").collect();
        if key.len() >= 1 {
            ret.push_str(key[0]);
        }
        ret.push_str(&") ");
        if key.len() >= 2 {
            ret.push_str("by ");
            ret.push_str(key[1]);
        }
        ret.push_str(&format!(
            "{} in {}.",
            agg_result.condition_op_num,
            rule.yaml["timeframe"].as_str().unwrap_or(""),
        ));
        return ret;
    }
    pub fn print_rule_load_info(
        rc: &HashMap<String, u128>,
        parseerror_count: &u128,
        ignore_count: &u128,
    ) {
        let mut total = parseerror_count + ignore_count;
        rc.into_iter().for_each(|(key, value)| {
            println!("{} rules: {}", key, value);
            total += value;
        });
        println!("Ignored rules: {}", ignore_count);
        println!("Rule parsing errors: {}", parseerror_count);
        println!("Total detection rules: {}", total);
        println!("");
    }
}

#[test]
fn test_parse_rule_files() {
    let level = "informational";
    let opt_rule_path = Some("./test_files/rules/level_yaml");
    let cole = Detection::parse_rule_files(level.to_owned(), opt_rule_path, &filter::exclude_ids());
    assert_eq!(5, cole.len());
}
