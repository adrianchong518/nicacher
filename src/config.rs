use std::{collections::BTreeSet, fmt, marker::PhantomData, path::PathBuf, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use anyhow::Context as _;

use crate::nix;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(deserialize_with = "set_string_or_struct")]
    pub upstreams: BTreeSet<nix::PriorityUpstream>,

    pub channel_url: Url,
    pub channels: Vec<nix::Channel>,

    pub local_data_path: PathBuf,
    pub database_max_connections: u32,
}

impl Config {
    const ENV_VAR: &str = "NICACHER_CONFIG";

    pub fn get() -> Self {
        tracing::info!("Reading config from env");

        let config = (|| -> anyhow::Result<Config> {
            let config_path = std::env::var(Self::ENV_VAR)?;
            let config_str = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Unable to read config from {config_path:?}"))?;

            Ok(toml::from_str::<Config>(&config_str)?)
        })()
        .unwrap_or_else(|e| {
            tracing::warn!("Unable to read config from env: {e}");
            tracing::info!("Using default config");
            Config::default()
        });

        tracing::trace!("Using config: {config:#?}");

        config
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            upstreams: [nix::PriorityUpstream::from_url(
                Url::parse("https://cache.nixos.org/").unwrap(),
            )]
            .into(),
            channel_url: Url::parse("https://channels.nixos.org/").unwrap(),
            channels: vec![nix::Channel::NixpkgsUnstable()],
            local_data_path: ".".into(),
            database_max_connections: 20,
        }
    }
}

fn set_string_or_struct<'de, T, D>(deserializer: D) -> Result<BTreeSet<T>, D::Error>
where
    T: Deserialize<'de> + FromStr + Ord,
    <T as FromStr>::Err: fmt::Display,
    D: Deserializer<'de>,
{
    use serde::de;

    // This is a Visitor that forwards string types to T's `FromStr` impl and
    // forwards map types to T's `Deserialize` impl. The `PhantomData` is to
    // keep the compiler from complaining about T being an unused generic type
    // parameter. We need T in order to know the Value type for the Visitor
    // impl.
    struct StringOrStruct<T>(PhantomData<fn() -> T>);

    impl<'de, T> de::Visitor<'de> for StringOrStruct<T>
    where
        T: Deserialize<'de> + FromStr,
        <T as FromStr>::Err: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string or map")
        }

        fn visit_str<E>(self, value: &str) -> Result<T, E>
        where
            E: de::Error,
        {
            FromStr::from_str(value).map_err(de::Error::custom)
        }

        fn visit_map<M>(self, map: M) -> Result<T, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            // `MapAccessDeserializer` is a wrapper that turns a `MapAccess`
            // into a `Deserializer`, allowing it to be used as the input to T's
            // `Deserialize` implementation. T then deserializes itself using
            // the entries from the map visitor.
            Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    // This is a common trick that enables passing a Visitor to the
    // `seq.next_element` call below.
    impl<'de, T> de::DeserializeSeed<'de> for StringOrStruct<T>
    where
        T: Deserialize<'de> + FromStr,
        <T as FromStr>::Err: fmt::Display,
    {
        type Value = T;

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(self)
        }
    }

    struct SetStringOrStruct<T>(PhantomData<T>);

    impl<'de, T> de::Visitor<'de> for SetStringOrStruct<T>
    where
        T: Deserialize<'de> + FromStr + Ord,
        <T as FromStr>::Err: fmt::Display,
    {
        type Value = BTreeSet<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("sequence of strings or maps")
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: de::SeqAccess<'de>,
        {
            let mut set = BTreeSet::new();
            // Tell it which Visitor to use by passing one in.
            while let Some(element) = seq.next_element_seed(StringOrStruct(PhantomData))? {
                set.insert(element);
            }
            Ok(set)
        }
    }

    deserializer.deserialize_any(SetStringOrStruct(PhantomData))
}
