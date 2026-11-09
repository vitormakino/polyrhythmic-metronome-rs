//! Domain model: tempo, pulses and the pattern they form.

use serde::{Deserialize, Serialize};

/// Palette used cyclically by pulse index.
const PALETTE: [[u8; 3]; 6] = [
    [0xf9, 0x73, 0x16],
    [0x3b, 0x82, 0xf6],
    [0x10, 0xb9, 0x81],
    [0xa8, 0x55, 0xf7],
    [0xf4, 0x3f, 0x5e],
    [0xea, 0xb3, 0x08],
];

/// Tempo in beats per minute, always within `MIN..=MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Bpm(u16);

impl Bpm {
    pub const MIN: u16 = 40;
    pub const MAX: u16 = 300;

    /// Builds a tempo, clamping to `MIN..=MAX`.
    pub fn new(v: i32) -> Bpm {
        Bpm(v.clamp(i32::from(Self::MIN), i32::from(Self::MAX)) as u16)
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

/// Accent level of a single beat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intensity {
    Mute,
    Soft,
    Medium,
    Hard,
}

impl Intensity {
    pub fn gain(self) -> f32 {
        match self {
            Intensity::Mute => 0.0,
            Intensity::Soft => 0.3,
            Intensity::Medium => 0.6,
            Intensity::Hard => 1.0,
        }
    }

    /// Click cycle: Mute -> Soft -> Medium -> Hard -> Mute.
    pub fn next(self) -> Intensity {
        match self {
            Intensity::Mute => Intensity::Soft,
            Intensity::Soft => Intensity::Medium,
            Intensity::Medium => Intensity::Hard,
            Intensity::Hard => Intensity::Mute,
        }
    }

    /// Maps the web app's continuous value to the nearest level (0.5 -> Medium).
    pub fn from_web(v: f32) -> Intensity {
        const LEVELS: [Intensity; 4] = [
            Intensity::Mute,
            Intensity::Soft,
            Intensity::Medium,
            Intensity::Hard,
        ];
        // The web app's 0.5 sits between Soft (0.3) and Medium (0.6) gains
        // but is documented to mean Medium, so it is pinned explicitly.
        if v == 0.5 {
            return Intensity::Medium;
        }
        let mut best = LEVELS[0];
        let mut best_dist = f32::INFINITY;
        for level in LEVELS {
            let dist = (level.gain() - v).abs();
            if dist < best_dist {
                best = level;
                best_dist = dist;
            }
        }
        best
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundType {
    Sine,
    Wood,
    Bell,
    Electronic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PulseId(u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pulse {
    pub id: PulseId,
    /// Number of beats per cycle, at least 1.
    pub beats: u32,
    pub frequency_hz: f32,
    pub color: [u8; 3],
    pub intensities: Vec<Intensity>,
    pub sound: SoundType,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub pulses: Vec<Pulse>,
    pub next_id: u64,
}

impl Default for Pattern {
    fn default() -> Self {
        use Intensity::{Hard, Medium};
        Pattern {
            pulses: vec![
                Pulse {
                    id: PulseId(0),
                    beats: 4,
                    frequency_hz: 800.0,
                    color: PALETTE[0],
                    intensities: vec![Hard, Medium, Medium, Medium],
                    sound: SoundType::Sine,
                    enabled: true,
                },
                Pulse {
                    id: PulseId(1),
                    beats: 3,
                    frequency_hz: 500.0,
                    color: PALETTE[1],
                    intensities: vec![Hard, Medium, Medium],
                    sound: SoundType::Wood,
                    enabled: true,
                },
            ],
            next_id: 2,
        }
    }
}

impl Pattern {
    /// Beat count of the first pulse (the reference cycle), or 0 if empty.
    pub fn reference_beats(&self) -> u32 {
        self.pulses.first().map_or(0, |p| p.beats)
    }

    /// Appends a pulse with `last.beats + 1` beats, all Medium, Sine.
    pub fn add_pulse(&mut self) -> PulseId {
        let index = self.pulses.len();
        let beats = self.pulses.last().map_or(4, |p| p.beats + 1);
        let id = PulseId(self.next_id);
        self.next_id += 1;
        self.pulses.push(Pulse {
            id,
            beats,
            // Deterministic stand-in for the web app's random frequency.
            frequency_hz: 400.0 + ((index as f32 * 137.0) % 400.0),
            color: PALETTE[index % PALETTE.len()],
            intensities: vec![Intensity::Medium; beats as usize],
            sound: SoundType::Sine,
            enabled: true,
        });
        id
    }

    /// Removes a pulse; the last remaining pulse cannot be removed.
    pub fn remove(&mut self, id: PulseId) -> bool {
        if self.pulses.len() <= 1 {
            return false;
        }
        match self.pulses.iter().position(|p| p.id == id) {
            Some(i) => {
                self.pulses.remove(i);
                true
            }
            None => false,
        }
    }

    /// Resizes a pulse (minimum 1 beat); new beats are Medium.
    pub fn set_beats(&mut self, id: PulseId, beats: u32) {
        if let Some(p) = self.pulse_mut(id) {
            let beats = beats.max(1);
            p.beats = beats;
            p.intensities.resize(beats as usize, Intensity::Medium);
        }
    }

    pub fn cycle_intensity(&mut self, id: PulseId, beat: usize) {
        if let Some(slot) = self.pulse_mut(id).and_then(|p| p.intensities.get_mut(beat)) {
            *slot = slot.next();
        }
    }

    pub fn set_sound(&mut self, id: PulseId, sound: SoundType) {
        if let Some(p) = self.pulse_mut(id) {
            p.sound = sound;
        }
    }

    pub fn set_enabled(&mut self, id: PulseId, enabled: bool) {
        if let Some(p) = self.pulse_mut(id) {
            p.enabled = enabled;
        }
    }

    /// Moves the pulse at `from` so it ends up at index `to`. Out of range is a no-op.
    pub fn move_pulse(&mut self, from: usize, to: usize) {
        if from < self.pulses.len() && to < self.pulses.len() {
            let p = self.pulses.remove(from);
            self.pulses.insert(to, p);
        }
    }

    pub fn pulse(&self, id: PulseId) -> Option<&Pulse> {
        self.pulses.iter().find(|p| p.id == id)
    }

    fn pulse_mut(&mut self, id: PulseId) -> Option<&mut Pulse> {
        self.pulses.iter_mut().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Intensity::{Hard, Medium, Mute, Soft};

    #[test]
    fn default_matches_web() {
        let p = Pattern::default();
        assert_eq!(p.pulses.len(), 2);
        let a = &p.pulses[0];
        assert_eq!(a.beats, 4);
        assert_eq!(a.frequency_hz, 800.0);
        assert_eq!(a.sound, SoundType::Sine);
        assert_eq!(a.intensities, vec![Hard, Medium, Medium, Medium]);
        assert_eq!(a.color, [0xf9, 0x73, 0x16]);
        let b = &p.pulses[1];
        assert_eq!(b.beats, 3);
        assert_eq!(b.frequency_hz, 500.0);
        assert_eq!(b.sound, SoundType::Wood);
        assert_eq!(b.intensities, vec![Hard, Medium, Medium]);
        assert_eq!(b.color, [0x3b, 0x82, 0xf6]);
        assert_eq!(p.reference_beats(), 4);
    }

    #[test]
    fn bpm_clamps() {
        assert_eq!(Bpm::new(10).get(), 40);
        assert_eq!(Bpm::new(999).get(), 300);
        assert_eq!(Bpm::new(-5).get(), 40);
        assert_eq!(Bpm::new(120).get(), 120);
    }

    #[test]
    fn intensity_cycle() {
        assert_eq!(Mute.next(), Soft);
        assert_eq!(Soft.next(), Medium);
        assert_eq!(Medium.next(), Hard);
        assert_eq!(Hard.next(), Mute);
        assert_eq!(Intensity::from_web(0.5), Medium);
        assert_eq!(Intensity::from_web(1.0), Hard);
        assert_eq!(Intensity::from_web(0.0), Mute);
        assert_eq!(Intensity::from_web(0.3), Soft);
        assert_eq!(Hard.gain(), 1.0);
        assert_eq!(Soft.gain(), 0.3);
    }

    #[test]
    fn add_pulse_follows_web() {
        let mut p = Pattern::default();
        let id = p.add_pulse();
        let n = p.pulse(id).unwrap();
        assert_eq!(n.beats, 4);
        assert!(n.intensities.iter().all(|&i| i == Medium));
        assert_eq!(n.intensities.len(), 4);
        assert_eq!(n.sound, SoundType::Sine);
        assert_eq!(n.color, [0x10, 0xb9, 0x81]);
        assert_eq!(n.frequency_hz, 400.0 + ((2.0_f32 * 137.0) % 400.0));
        assert_ne!(id, PulseId(0));
        assert_ne!(id, PulseId(1));
    }

    #[test]
    fn set_beats_grows_with_medium_and_truncates() {
        let mut p = Pattern::default();
        let id = p.pulses[0].id;
        p.set_beats(id, 6);
        assert_eq!(
            p.pulse(id).unwrap().intensities,
            vec![Hard, Medium, Medium, Medium, Medium, Medium]
        );
        p.set_beats(id, 2);
        assert_eq!(p.pulse(id).unwrap().intensities, vec![Hard, Medium]);
        p.set_beats(id, 0);
        let n = p.pulse(id).unwrap();
        assert_eq!(n.beats, 1);
        assert_eq!(n.intensities, vec![Hard]);
    }

    #[test]
    fn cannot_remove_last_pulse() {
        let mut p = Pattern::default();
        p.add_pulse();
        let ids: Vec<_> = p.pulses.iter().map(|x| x.id).collect();
        assert!(p.remove(ids[0]));
        assert!(p.remove(ids[1]));
        assert!(!p.remove(ids[2]));
        assert_eq!(p.pulses.len(), 1);
    }

    #[test]
    fn move_changes_reference() {
        let mut p = Pattern::default();
        p.move_pulse(1, 0);
        assert_eq!(p.reference_beats(), 3);
    }

    #[test]
    fn unknown_id_and_bad_index_are_noops() {
        let mut p = Pattern::default();
        let before = p.clone();
        let ghost = PulseId(99);
        p.set_beats(ghost, 7);
        p.cycle_intensity(ghost, 0);
        p.set_sound(ghost, SoundType::Bell);
        p.set_enabled(ghost, false);
        assert!(!p.remove(ghost));
        p.cycle_intensity(p.pulses[0].id, 10);
        p.move_pulse(5, 0);
        assert_eq!(p, before);
    }

    #[test]
    fn cycle_intensity_changes_beat() {
        let mut p = Pattern::default();
        let id = p.pulses[0].id;
        p.cycle_intensity(id, 0);
        assert_eq!(p.pulse(id).unwrap().intensities[0], Mute);
    }
}
