//! `SoundbanksInfo.xml`: which bank holds an event, its streamed media and its duration.
//!
//! Wwise writes this file when it generates banks. We use it to find what the target cue
//! needs (its bank and streamed `.wem` IDs) and what to expect from it (duration, maximum
//! attenuation distance). The game itself doesn't read it at runtime; it loads fixed bank
//! chunks (see [`crate::settings::PRELOAD_BANKS`]).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// One event as listed under a bank's `IncludedEvents`.
#[derive(Debug, Clone, PartialEq)]
pub struct EventInfo {
    /// Short ID (`Id`).
    pub id: u32,
    /// `Name`.
    pub name: String,
    /// `ShortName` of every bank that includes the event.
    pub banks: Vec<String>,
    /// `MaxAttenuation` (metres) if the event has distance attenuation.
    pub max_attenuation: Option<f32>,
    /// `DurationType` (`OneShot`, `Infinite`, `Mixed`, `Unknown`).
    pub duration_type: Option<String>,
    /// `DurationMin`, seconds.
    pub duration_min: Option<f32>,
    /// `DurationMax`, seconds.
    pub duration_max: Option<f32>,
    /// IDs of streamed media (`ReferencedStreamedFiles/File/@Id`).
    pub streamed_files: Vec<u32>,
}

/// Index of every event by name.
#[derive(Debug, Clone, Default)]
pub struct BankInfo {
    events: HashMap<String, EventInfo>,
    banks: Vec<(u32, String)>,
}

fn attr(e: &BytesStart<'_>, key: &str) -> Result<Option<String>> {
    for a in e.attributes() {
        let a = a?;
        if a.key.as_ref() == key {
            // The attributes we read (IDs, names, numbers) never contain XML escapes.
            return Ok(Some(a.value.into_owned()));
        }
    }
    Ok(None)
}

fn parse_f32(s: Option<String>) -> Result<Option<f32>> {
    // Wwise writes "25." for whole numbers; Rust's parser accepts that.
    s.map(|s| s.parse::<f32>().with_context(|| format!("bad float {s:?}"))).transpose()
}

impl BankInfo {
    /// Parses the XML text.
    ///
    /// # Errors
    ///
    /// On malformed XML or attribute values.
    pub fn parse(xml: &str) -> Result<Self> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut info = Self::default();
        let mut stack: Vec<String> = Vec::new();
        let mut bank_id: u32 = 0;
        let mut bank_name: Option<String> = None;
        let mut current: Option<EventInfo> = None;
        // Events of the current bank, resolved at `</SoundBank>` (Wwise writes ShortName
        // first, but don't rely on it).
        let mut pending: Vec<EventInfo> = Vec::new();

        loop {
            match reader.read_event()? {
                Event::Start(e) => {
                    let name = e.name().as_ref().to_owned();
                    match name.as_str() {
                        "SoundBank" => {
                            bank_id = attr(&e, "Id")?.context("SoundBank without Id")?.parse()?;
                            bank_name = None;
                        }
                        "Event" if stack.last().is_some_and(|p| p == "IncludedEvents") => {
                            current = Some(Self::event_from(&e)?);
                        }
                        "File"
                            if current.is_some()
                                && stack.last().is_some_and(|p| p == "ReferencedStreamedFiles") =>
                        {
                            if let Some(ev) = current.as_mut() {
                                ev.streamed_files
                                    .push(attr(&e, "Id")?.context("File without Id")?.parse()?);
                            }
                        }
                        _ => {}
                    }
                    stack.push(name);
                }
                Event::Empty(e) => {
                    if e.name().as_ref() == "Event"
                        && stack.last().is_some_and(|p| p == "IncludedEvents")
                    {
                        pending.push(Self::event_from(&e)?);
                    } else if e.name().as_ref() == "File"
                        && current.is_some()
                        && stack.last().is_some_and(|p| p == "ReferencedStreamedFiles")
                        && let Some(ev) = current.as_mut()
                    {
                        ev.streamed_files
                            .push(attr(&e, "Id")?.context("File without Id")?.parse()?);
                    }
                }
                Event::Text(t) => {
                    let parent_is_bank = stack.len() >= 2 && stack[stack.len() - 2] == "SoundBank";
                    if parent_is_bank && stack.last().is_some_and(|p| p == "ShortName") {
                        let name = t.xml10_content().into_owned();
                        info.banks.push((bank_id, name.clone()));
                        bank_name = Some(name);
                    }
                }
                Event::End(e) => {
                    let Some(top) = stack.pop() else { bail!("unbalanced XML") };
                    if top != e.name().as_ref() {
                        bail!("mismatched end tag");
                    }
                    if top == "Event"
                        && let Some(ev) = current.take()
                    {
                        pending.push(ev);
                    }
                    // Resolve at the end of the bank, when its ShortName is surely known.
                    if top == "SoundBank" {
                        let bank = bank_name.clone().unwrap_or_default();
                        for mut ev in pending.drain(..) {
                            match info.events.get_mut(&ev.name) {
                                // The same event can be listed in several banks, with media
                                // in only some of them: merge, don't drop.
                                Some(existing) => {
                                    existing.banks.push(bank.clone());
                                    for f in ev.streamed_files {
                                        if !existing.streamed_files.contains(&f) {
                                            existing.streamed_files.push(f);
                                        }
                                    }
                                    existing.max_attenuation =
                                        existing.max_attenuation.or(ev.max_attenuation);
                                    existing.duration_type =
                                        existing.duration_type.take().or(ev.duration_type);
                                    existing.duration_min =
                                        existing.duration_min.or(ev.duration_min);
                                    existing.duration_max =
                                        existing.duration_max.or(ev.duration_max);
                                }
                                None => {
                                    ev.banks.push(bank.clone());
                                    info.events.insert(ev.name.clone(), ev);
                                }
                            }
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(info)
    }

    fn event_from(e: &BytesStart<'_>) -> Result<EventInfo> {
        Ok(EventInfo {
            id: attr(e, "Id")?.context("Event without Id")?.parse()?,
            name: attr(e, "Name")?.context("Event without Name")?,
            banks: Vec::new(),
            max_attenuation: parse_f32(attr(e, "MaxAttenuation")?)?,
            duration_type: attr(e, "DurationType")?,
            duration_min: parse_f32(attr(e, "DurationMin")?)?,
            duration_max: parse_f32(attr(e, "DurationMax")?)?,
            streamed_files: Vec::new(),
        })
    }

    /// Reads and parses the file.
    ///
    /// # Errors
    ///
    /// If it can't be read or parsed.
    pub fn load(path: &Path) -> Result<Self> {
        let xml =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&xml).with_context(|| format!("parsing {}", path.display()))
    }

    /// An event by name.
    #[must_use]
    pub fn event(&self, name: &str) -> Option<&EventInfo> {
        self.events.get(name)
    }

    /// Number of distinct events.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Every bank as `(id, ShortName)`.
    #[must_use]
    pub fn banks(&self) -> &[(u32, String)] {
        &self.banks
    }
}

#[cfg(test)]
mod tests {
    use super::BankInfo;
    use crate::shortid::short_id;

    // Shape copied from the game's file (SchemaVersion 12), trimmed.
    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<SoundBanksInfo Platform="Mac" BasePlatform="Mac" SchemaVersion="12" SoundbankVersion="135">
  <StreamedFiles>
    <File Id="957188610" Language="SFX"><ShortName>x.wav</ShortName><Path>x.wem</Path></File>
  </StreamedFiles>
  <SoundBanks>
    <SoundBank Id="247461405" GUID="{X}" Language="SFX">
      <ObjectPath>\SoundBanks\Dialogs_Prologue_ENG</ObjectPath>
      <ShortName>Dialogs_Prologue_ENG</ShortName>
      <Path>Dialogs_Prologue_ENG.bnk</Path>
      <IncludedEvents>
        <Event Id="249985486" Name="CH0_MeetSeelahAnevia_01" MaxAttenuation="25." DurationType="OneShot" DurationMin="3.984854" DurationMax="3.984854">
          <ReferencedStreamedFiles>
            <File Id="957188610" Language="SFX">
              <ShortName>Voices\English\CH0_MeetSeelahAnevia_01.wav</ShortName>
            </File>
          </ReferencedStreamedFiles>
          <AuxBusSends><AuxBus Id="1" Name="Delay"/></AuxBusSends>
        </Event>
        <Event Id="2" Name="TECH_Empty" DurationType="Unknown"/>
      </IncludedEvents>
    </SoundBank>
    <SoundBank Id="3161908922" GUID="{Y}" Language="SFX">
      <ShortName>Main</ShortName>
      <IncludedEvents><Event Id="2" Name="TECH_Empty" DurationType="Unknown"/></IncludedEvents>
    </SoundBank>
  </SoundBanks>
</SoundBanksInfo>"#;

    #[test]
    fn indexes_events_with_bank_and_media() {
        let info = BankInfo::parse(SAMPLE).unwrap();
        let ev = info.event("CH0_MeetSeelahAnevia_01").unwrap();
        assert_eq!(ev.id, short_id(&ev.name));
        assert_eq!(ev.banks, ["Dialogs_Prologue_ENG"]);
        assert_eq!(ev.max_attenuation, Some(25.0));
        assert_eq!(ev.duration_type.as_deref(), Some("OneShot"));
        assert!((ev.duration_max.unwrap() - 3.984_854).abs() < 1e-6);
        assert_eq!(ev.streamed_files, [957_188_610]);
    }

    #[test]
    fn duplicate_events_merge_media_and_late_short_names_work() {
        // Shape seen in the game's file: an event listed first without media, then with.
        let xml = r#"<SoundBanksInfo><SoundBanks>
          <SoundBank Id="1"><ShortName>A</ShortName><IncludedEvents>
            <Event Id="9" Name="E" DurationType="Infinite"/>
          </IncludedEvents></SoundBank>
          <SoundBank Id="2"><IncludedEvents>
            <Event Id="9" Name="E" MaxAttenuation="40.">
              <ReferencedStreamedFiles><File Id="5"/><File Id="6"/></ReferencedStreamedFiles>
            </Event>
          </IncludedEvents><ShortName>B</ShortName></SoundBank>
        </SoundBanks></SoundBanksInfo>"#;
        let info = BankInfo::parse(xml).unwrap();
        let ev = info.event("E").unwrap();
        assert_eq!(ev.banks, ["A", "B"]);
        assert_eq!(ev.streamed_files, [5, 6]);
        assert_eq!(ev.max_attenuation, Some(40.0));
        assert_eq!(ev.duration_type.as_deref(), Some("Infinite"));
    }

    #[test]
    fn events_in_several_banks_list_them_all() {
        let info = BankInfo::parse(SAMPLE).unwrap();
        assert_eq!(info.event_count(), 2);
        assert_eq!(info.event("TECH_Empty").unwrap().banks, ["Dialogs_Prologue_ENG", "Main"]);
        assert_eq!(info.banks().len(), 2);
    }
}
