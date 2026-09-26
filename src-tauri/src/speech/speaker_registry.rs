//! Known-speaker matching.
//!
//! Each enrolled person has one or more embeddings. A voice is scored
//! against a person as the mean of their top-3 similarities (robust to one
//! noisy enrollment sample). Thresholds come from settings and must be
//! calibrated for the embedding model; they are not universal confidence
//! levels. Speaker identity is never treated as certain.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::embeddings::cosine;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SpeakerIdentity {
    Known {
        #[serde(rename = "personId")]
        person_id: String,
        confidence: f32,
    },
    Possible {
        #[serde(rename = "personId")]
        person_id: String,
        confidence: f32,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct MatchThresholds {
    pub known: f32,
    pub possible: f32,
    /// Required lead of the best person over the runner-up for "known".
    pub margin: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    people: Vec<(String, Vec<Vec<f32>>)>,
}

impl Registry {
    pub fn add(&mut self, person_id: &str, embeddings: Vec<Vec<f32>>) {
        if !embeddings.is_empty() {
            self.people.push((person_id.to_string(), embeddings));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.people.is_empty()
    }

    pub fn score(embeddings: &[Vec<f32>], probe: &[f32]) -> f32 {
        let mut sims: Vec<f32> = embeddings.iter().map(|e| cosine(e, probe)).collect();
        sims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let top = &sims[..sims.len().min(3)];
        if top.is_empty() {
            0.0
        } else {
            top.iter().sum::<f32>() / top.len() as f32
        }
    }

    /// Best matches, highest first.
    pub fn rank(&self, probe: &[f32]) -> Vec<(String, f32)> {
        let mut out: Vec<(String, f32)> = self.people.iter().map(|(id, e)| (id.clone(), Self::score(e, probe))).collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// `exclude` removes people already assigned with higher confidence to
    /// another cluster in the same meeting.
    pub fn identify(&self, probe: &[f32], t: MatchThresholds, exclude: &[String]) -> SpeakerIdentity {
        let ranked: Vec<(String, f32)> = self.rank(probe).into_iter().filter(|(id, _)| !exclude.contains(id)).collect();
        let Some((best, score)) = ranked.first().cloned() else { return SpeakerIdentity::Unknown };
        let runner_up = ranked.get(1).map(|r| r.1).unwrap_or(-1.0);
        if score >= t.known && score - runner_up >= t.margin {
            SpeakerIdentity::Known { person_id: best, confidence: score }
        } else if score >= t.possible {
            SpeakerIdentity::Possible { person_id: best, confidence: score }
        } else {
            SpeakerIdentity::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(angle: f32) -> Vec<f32> {
        vec![angle.cos(), angle.sin()]
    }

    const T: MatchThresholds = MatchThresholds { known: 0.9, possible: 0.7, margin: 0.05 };

    #[test]
    fn identifies_known_possible_unknown() {
        let mut r = Registry::default();
        r.add("tom", vec![unit(0.0), unit(0.05)]);
        r.add("sarah", vec![unit(1.5)]);
        assert!(matches!(r.identify(&unit(0.02), T, &[]), SpeakerIdentity::Known { ref person_id, .. } if person_id == "tom"));
        assert!(matches!(r.identify(&unit(0.6), T, &[]), SpeakerIdentity::Possible { ref person_id, .. } if person_id == "tom"));
        assert_eq!(r.identify(&unit(-1.0), T, &[]), SpeakerIdentity::Unknown);
    }

    #[test]
    fn ambiguous_matches_are_only_possible() {
        let mut r = Registry::default();
        r.add("a", vec![unit(0.0)]);
        r.add("b", vec![unit(0.02)]);
        // Both score above "known" but too close to each other.
        assert!(matches!(r.identify(&unit(0.01), T, &[]), SpeakerIdentity::Possible { .. }));
    }

    #[test]
    fn excluded_people_are_skipped() {
        let mut r = Registry::default();
        r.add("tom", vec![unit(0.0)]);
        assert_eq!(r.identify(&unit(0.0), T, &["tom".into()]), SpeakerIdentity::Unknown);
    }

    #[test]
    fn serializes_as_tagged_union() {
        let v = serde_json::to_value(SpeakerIdentity::Known { person_id: "p".into(), confidence: 0.9 }).unwrap();
        assert_eq!(v["type"], "known");
        assert_eq!(v["personId"], "p");
    }
}
