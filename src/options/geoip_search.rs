use compact_str::CompactString;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use maxminddb::{geoip2, MaxMindDBError, Reader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::{net::IpAddr, str::FromStr};

lazy_static! {
    pub static ref IP_MAP: Mutex<HashMap<IpAddr, CompactString>> = Mutex::new(HashMap::new());
}
pub struct GeoIPSearch {
    pub asn_reader: Reader<Vec<u8>>,
    pub country_reader: Reader<Vec<u8>>,
    pub city_reader: Reader<Vec<u8>>,
}

impl GeoIPSearch {
    pub fn new(path: &Path, asn_country_city_filename: Vec<&str>) -> GeoIPSearch {
        GeoIPSearch {
            asn_reader: maxminddb::Reader::open_readfile(path.join(asn_country_city_filename[0]))
                .unwrap(),
            country_reader: maxminddb::Reader::open_readfile(
                path.join(asn_country_city_filename[1]),
            )
            .unwrap(),
            city_reader: maxminddb::Reader::open_readfile(path.join(asn_country_city_filename[2]))
                .unwrap(),
        }
    }

    /// check existence files in specified path by geo-ip option.
    pub fn check_exist_geo_ip_files(
        geo_ip_dir_path: &Option<PathBuf>,
        check_files: Vec<&str>,
    ) -> Result<Option<PathBuf>, String> {
        if let Some(path) = geo_ip_dir_path {
            let mut combined_err = vec![];
            for file_name in check_files {
                let mmdb_path = path.join(file_name);
                if !mmdb_path.exists() {
                    combined_err.push(format!(
                        "Cannot find the appropriate MaxMind GeoIP database files. filepath: {mmdb_path:?}"
                    ));
                }
            }
            if combined_err.is_empty() {
                Ok(geo_ip_dir_path.to_owned())
            } else {
                Err(combined_err.join("\n"))
            }
        } else {
            Ok(None)
        }
    }

    /// convert IP address string to geo data
    pub fn convert_ip_to_geo(&self, target_ip: &str) -> Result<String, MaxMindDBError> {
        let addr = IpAddr::from_str(target_ip).unwrap();

        // If the IP address is the same, the result obtained is the same, so the lookup process is omitted by obtaining the result of a hit from the cache.
        if let Some(cached_data) = IP_MAP.lock().unwrap().get(&addr) {
            return Ok(cached_data.to_string());
        }

        let asn: geoip2::Asn = self.asn_reader.lookup(addr)?;
        let country: geoip2::Country = self.country_reader.lookup(addr)?;
        let city: geoip2::City = self.city_reader.lookup(addr)?;
        let geo_data = format!("{asn:#?}🦅{country:#?}🦅{city:#?}");
        IP_MAP
            .lock()
            .unwrap()
            .insert(addr, CompactString::from(&geo_data));
        Ok(geo_data)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::GeoIPSearch;

    #[test]
    fn test_no_specified_geo_ip_option() {
        assert!(GeoIPSearch::check_exist_geo_ip_files(
            &None,
            vec![
                "GeoLite2-ASN.mmdb",
                "GeoLite2-Country.mmdb",
                "GeoLite2-City.mmdb",
            ]
        )
        .unwrap()
        .is_none())
    }

    #[test]
    fn test_not_exist_files() {
        let target_files = vec![
            "GeoLite2-NoExist1.mmdb",
            "GeoLite2-NoExist2.mmdb",
            "GeoLite2-NoExist3.mmdb",
        ];
        let test_path = Path::new("test_files/mmdb").to_path_buf();
        let mut expect_err_msg = vec![];
        for file_path in &target_files {
            expect_err_msg.push(format!(
                "Cannot find the appropriate MaxMind GeoIP database files. filepath: {:?}",
                test_path.join(file_path)
            ));
        }
        assert_eq!(
            GeoIPSearch::check_exist_geo_ip_files(&Some(test_path), target_files),
            Err(expect_err_msg.join("\n"))
        )
    }
}
