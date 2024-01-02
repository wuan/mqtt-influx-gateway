use std::fmt;
use std::fmt::Debug;
use std::sync::mpsc::SyncSender;

use influxdb::{Timestamp, WriteQuery};
use paho_mqtt::Message;
use serde::{Deserialize, Serialize};

use crate::data::{CheckMessage, shelly};
use crate::WriteType;

pub trait Timestamped {
    fn timestamp(&self) -> Option<i64>;
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SwitchData {
    pub(crate) output: bool,
    #[serde(rename = "apower")]
    pub(crate) power: f32,
    pub(crate) voltage: f32,
    pub(crate) current: f32,
    #[serde(rename = "aenergy")]
    pub(crate) energy: EnergyData,
    pub(crate) temperature: TemperatureData,
}

impl Timestamped for SwitchData {
    fn timestamp(&self) -> Option<i64> {
        self.energy.minute_ts
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CoverData {
    #[serde(rename = "current_pos")]
    pub(crate) position: Option<i32>,
    #[serde(rename = "apower")]
    pub(crate) power: f32,
    pub(crate) voltage: f32,
    pub(crate) current: f32,
    #[serde(rename = "aenergy")]
    pub(crate) energy: EnergyData,
    pub(crate) temperature: TemperatureData,
}

impl Timestamped for CoverData {
    fn timestamp(&self) -> Option<i64> {
        self.energy.minute_ts
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EnergyData {
    pub(crate) total: f32,
    pub(crate) minute_ts: Option<i64>,
}

impl fmt::Debug for EnergyData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} Wh", self.total)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TemperatureData {
    #[serde(rename = "tC")]
    pub(crate) t_celsius: f32,
}

impl fmt::Debug for TemperatureData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} °C", self.t_celsius)
    }
}

pub struct ShellyLogger {
    tx: SyncSender<WriteQuery>,
}

impl ShellyLogger {
    pub(crate) fn new(tx: SyncSender<WriteQuery>) -> Self {
        ShellyLogger { tx }
    }
}

pub fn parse<'a, T: Deserialize<'a> + Clone>(msg: &'a Message) -> Result<Option<T>, &'static str> {
    let data = serde_json::from_slice::<T>(msg.payload()).map_err(|error| {
        eprintln!("{:?}", error);
        "could not deserialize JSON"
    })?;
    Ok(Some(data.clone()))
}

const SWITCH_FIELDS: &[(&str, fn(data: &SwitchData) -> Option<WriteType>, &str)] = &[
    ("output", |data: &SwitchData| Some(WriteType::Int(data.output as i32)), "bool"),
    ("power", |data: &SwitchData| Some(WriteType::Float(data.power)), "W"),
    ("current", |data: &SwitchData| Some(WriteType::Float(data.current)), "A"),
    ("voltage", |data: &SwitchData| Some(WriteType::Float(data.voltage)), "V"),
    ("total_energy", |data: &SwitchData| Some(WriteType::Float(data.energy.total)), "Wh"),
    ("temperature", |data: &SwitchData| Some(WriteType::Float(data.temperature.t_celsius)), "°C"),
];

const COVER_FIELDS: &[(&str, fn(data: &CoverData) -> Option<WriteType>, &str)] = &[
    ("position", |data: &CoverData| if let Some(position) = data.position { Some(WriteType::Int(position)) } else { None }, "%"),
    ("power", |data: &CoverData| Some(WriteType::Float(data.power)), "W"),
    ("current", |data: &CoverData| Some(WriteType::Float(data.current)), "A"),
    ("voltage", |data: &CoverData| Some(WriteType::Float(data.voltage)), "V"),
    ("total_energy", |data: &CoverData| Some(WriteType::Float(data.energy.total)), "Wh"),
    ("temperature", |data: &CoverData| Some(WriteType::Float(data.temperature.t_celsius)), "°C"),
];

impl CheckMessage for ShellyLogger {
    fn check_message(&mut self, msg: &Message) {
        let topic = msg.topic();
        if topic.ends_with("/status/switch:0") {
            handle_message(msg, &self.tx, SWITCH_FIELDS);
        } else if topic.ends_with("/status/cover:0") {
            handle_message(msg, &self.tx, COVER_FIELDS);
        }
    }
}

fn handle_message<'a, T: Deserialize<'a> + Clone + Debug + Timestamped>(msg: &'a Message, tx: &SyncSender<WriteQuery>, fields: &[(&str, fn(&T) -> Option<WriteType>, &str)]) {
    let location = msg.topic().split("/").nth(1).unwrap();
    let result: Option<T> = shelly::parse(&msg).unwrap();
    if let Some(data) = result {
        println!("Shelly {}: {:?}", location, data);

        if let Some(minute_ts) = data.timestamp() {
            let timestamp = Timestamp::Seconds(minute_ts as u128);
            for (measurement, value, unit) in fields {
                let query = WriteQuery::new(timestamp, *measurement);
                let result = value(&data);
                let query = match result {
                    Some(WriteType::Int(i)) => {
                        query.add_field("value", i)
                    }
                    Some(WriteType::Float(f)) => {
                        query.add_field("value", f)
                    }
                    None => {query}
                };

                let query = query.add_tag("location", location)
                    .add_tag("sensor", "shelly")
                    .add_tag("type", "switch")
                    .add_tag("unit", unit);
                tx.send(query).expect("failed to send");
            }
        } else {
            println!("{} no timestamp {:?}", msg.topic(), msg.payload_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use paho_mqtt::QOS_1;

    use super::*;

    #[test]
    fn test_parse_switch_status() -> Result<(), &'static str> {
        let message = Message::new("shellies/loo-fan/status/switch:0", "{\"id\":0, \"source\":\"timer\", \"output\":false, \"apower\":0.0, \"voltage\":226.5, \"current\":3.1, \"aenergy\":{\"total\":1094.865,\"by_minute\":[0.000,0.000,0.000],\"minute_ts\":1703415907},\"temperature\":{\"tC\":36.4, \"tF\":97.5}}", QOS_1);
        let result: SwitchData = parse(&message)?.unwrap();

        assert_eq!(result.output, false);
        assert_eq!(result.power, 0.0);
        assert_eq!(result.voltage, 226.5);
        assert_eq!(result.current, 3.1);
        assert_eq!(result.energy.total, 1094.865);
        assert_eq!(result.temperature.t_celsius, 36.4);

        Ok(())
    }

    #[test]
    fn test_parse_cover_status() -> Result<(), &'static str> {
        let message = Message::new("shellies/bedroom-curtain/status/cover:0", "{\"id\":0, \"source\":\"limit_switch\", \"state\":\"open\",\"apower\":0.0,\"voltage\":231.7,\"current\":0.500,\"pf\":0.00,\"freq\":50.0,\"aenergy\":{\"total\":3.143,\"by_minute\":[0.000,0.000,97.712],\"minute_ts\":1703414519},\"temperature\":{\"tC\":30.7, \"tF\":87.3},\"pos_control\":true,\"last_direction\":\"open\",\"current_pos\":100}", QOS_1);
        let result: CoverData = parse(&message)?.unwrap();

        assert_eq!(result.position, Some(100));
        assert_eq!(result.power, 0.0);
        assert_eq!(result.voltage, 231.7);
        assert_eq!(result.current, 0.5);
        assert_eq!(result.energy.total, 3.143);
        assert_eq!(result.temperature.t_celsius, 30.7);
        assert_eq!(result.energy.minute_ts.unwrap(), 1703414519);

        Ok(())
    }

    #[test]
    fn test_parse_cover_status_without_timestamp() -> Result<(), &'static str> {
        let message = Message::new("shellies/bedroom-curtain/status/cover:0", "{\"id\":0, \"source\":\"limit_switch\", \"state\":\"open\",\"apower\":0.0,\"voltage\":231.7,\"current\":0.500,\"pf\":0.00,\"freq\":50.0,\"aenergy\":{\"total\":3.143,\"by_minute\":[0.000,0.000,97.712]},\"temperature\":{\"tC\":30.7, \"tF\":87.3},\"pos_control\":true,\"last_direction\":\"open\",\"current_pos\":100}", QOS_1);
        let result: CoverData = parse(&message)?.unwrap();

        assert!(result.energy.minute_ts.is_none());

        Ok(())
    }

    #[test]
    fn test_parse_cover_status_without_position() -> Result<(), &'static str> {
        let message = Message::new("shellies/bedroom-curtain/status/cover:0", "{\"id\":0, \"source\":\"limit_switch\", \"state\":\"open\",\"apower\":0.0,\"voltage\":231.7,\"current\":0.500,\"pf\":0.00,\"freq\":50.0,\"aenergy\":{\"total\":3.143,\"by_minute\":[0.000,0.000,97.712],\"minute_ts\":1703414519},\"temperature\":{\"tC\":30.7, \"tF\":87.3},\"pos_control\":true,\"last_direction\":\"open\"}", QOS_1);
        let result: CoverData = parse(&message)?.unwrap();

        assert!(result.position.is_none());

        Ok(())
    }
}