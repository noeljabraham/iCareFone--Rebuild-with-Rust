use std::collections::HashMap;

use crate::domain::Device;

use super::command;

pub fn discover() -> Result<Vec<Device>, String> {
    if !command::available("idevice_id") || !command::available("ideviceinfo") {
        return Err("libimobiledevice tools are missing. Install `libimobiledevice` via Homebrew (`brew install libimobiledevice`) and reconnect your iOS device.".to_owned());
    }

    let output = command::checked("idevice_id", ["-l"])?;
    let mut devices = Vec::new();
    for udid in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        devices.push(read_device(udid));
    }
    Ok(devices)
}

fn read_device(udid: &str) -> Device {
    let info = command::checked("ideviceinfo", ["-u", udid]).unwrap_or_default();
    let values = parse_info(&info);

    let product_type = get(&values, "ProductType");
    let friendly_model = friendly_model_name(&product_type);

    let (battery_level, battery_state) = read_battery(udid);
    let (disk_total_bytes, disk_free_bytes) = read_disk_usage(udid);

    Device {
        udid: udid.to_owned(),
        name: get(&values, "DeviceName"),
        product_type,
        friendly_model,
        product_version: get(&values, "ProductVersion"),
        serial_number: get(&values, "SerialNumber"),
        battery_level,
        battery_state,
        disk_total_bytes,
        disk_free_bytes,
        connection: "USB / Local Pairing".to_owned(),
    }
}

fn read_battery(udid: &str) -> (Option<i32>, String) {
    if let Ok(info) = command::checked("ideviceinfo", ["-u", udid, "-q", "com.apple.mobile.battery"]) {
        let values = parse_info(&info);
        let level = values.get("BatteryCurrentCapacity").and_then(|val| val.parse::<i32>().ok());
        let charging = values.get("BatteryIsCharging").map(|val| val.as_str()) == Some("true");
        let fully_charged = values.get("FullyCharged").map(|val| val.as_str()) == Some("true");
        let state = if fully_charged {
            "Fully Charged".to_owned()
        } else if charging {
            "Charging".to_owned()
        } else {
            "Discharging".to_owned()
        };
        (level, state)
    } else {
        (None, String::new())
    }
}

fn read_disk_usage(udid: &str) -> (Option<u64>, Option<u64>) {
    if let Ok(info) = command::checked("ideviceinfo", ["-u", udid, "-q", "com.apple.disk_usage"]) {
        let values = parse_info(&info);
        let total = values.get("TotalDataCapacity").and_then(|val| val.parse::<u64>().ok());
        let free = values.get("AmountDataAvailable").and_then(|val| val.parse::<u64>().ok());
        (total, free)
    } else {
        (None, None)
    }
}

pub fn restart(udid: &str) -> Result<String, String> {
    if !command::available("idevicediagnostics") {
        return Err("idevicediagnostics is required to restart device".to_owned());
    }
    command::checked("idevicediagnostics", ["-u", udid, "restart"])?;
    Ok("Device restart command sent successfully".to_owned())
}

pub fn shutdown(udid: &str) -> Result<String, String> {
    if !command::available("idevicediagnostics") {
        return Err("idevicediagnostics is required to shutdown device".to_owned());
    }
    command::checked("idevicediagnostics", ["-u", udid, "shutdown"])?;
    Ok("Device shutdown command sent successfully".to_owned())
}

pub fn sleep(udid: &str) -> Result<String, String> {
    if !command::available("idevicediagnostics") {
        return Err("idevicediagnostics is required to sleep device".to_owned());
    }
    command::checked("idevicediagnostics", ["-u", udid, "sleep"])?;
    Ok("Device sleep command sent successfully".to_owned())
}

fn get(values: &HashMap<String, String>, key: &str) -> String {
    values.get(key).cloned().unwrap_or_default()
}

pub fn parse_info(input: &str) -> HashMap<String, String> {
    input
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

pub fn friendly_model_name(product_type: &str) -> String {
    match product_type {
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone11,8" => "iPhone XR",
        "iPhone11,2" => "iPhone XS",
        "iPhone11,4" | "iPhone11,6" => "iPhone XS Max",
        "iPhone10,3" | "iPhone10,6" => "iPhone X",
        "iPad13,18" | "iPad13,19" => "iPad (10th generation)",
        "iPad13,4" | "iPad13,5" | "iPad13,6" | "iPad13,7" => "iPad Pro 11-inch (3rd gen)",
        "iPad14,3" | "iPad14,4" => "iPad Pro 11-inch (4th gen)",
        _ => {
            if product_type.starts_with("iPhone") {
                "iPhone"
            } else if product_type.starts_with("iPad") {
                "iPad"
            } else if product_type.starts_with("iPod") {
                "iPod"
            } else {
                product_type
            }
        }
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_info_lines() {
        let input = "DeviceName: My iPhone\nProductType: iPhone15,2\nSerialNumber: F2LK\n";
        let map = parse_info(input);
        assert_eq!(map.get("DeviceName").unwrap(), "My iPhone");
        assert_eq!(map.get("ProductType").unwrap(), "iPhone15,2");
        assert_eq!(friendly_model_name("iPhone15,2"), "iPhone 14 Pro");
    }
}

