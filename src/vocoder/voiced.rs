//! Voiced spectrum synthesis.
//!
//! Computes the voiced signal s_v(n) from the enhanced spectral amplitudes,
//! phase terms, and voiced/unvoiced decisions. Uses a four-way blending
//! scheme to transition smoothly between voiced and unvoiced frames.

use std::cmp::max;
use std::f32::consts::PI;

use rand::Rng;

use super::consts::{MAX_HARMONICS, SAMPLES_PER_FRAME};
use super::descramble::VoiceDecisions;
use super::enhance::EnhancedSpectrals;
use super::params::BaseParams;
use super::prev::PrevFrame;
use super::window;

/// Base phase offsets Psi_l computed from previous and current frame parameters.
///
/// Accumulates phase across frames according to Eq 139 of TIA-102.BABA.
pub(crate) struct PhaseBase([f32; MAX_HARMONICS]);

impl PhaseBase {
    /// Create a new `PhaseBase` from the given current and previous frame parameters.
    ///
    /// Computes Psi_l = prev_phase_base(l) + scale * l for all harmonics,
    /// where scale = (w_0_prev + w_0) * N / 2 (Eq 139).
    pub(crate) fn new(params: &BaseParams, prev: &PrevFrame) -> Self {
        let mut base = [0.0; MAX_HARMONICS];

        // Compute common scaling factor in Eq 139.
        let scale = (prev.params.fundamental_frequency + params.fundamental_frequency)
            * SAMPLES_PER_FRAME as f32
            / 2.0;

        // Compute Eq 139 for each harmonic.
        for l in 1..=MAX_HARMONICS {
            base[l - 1] = prev.phase_base.get(l) + scale * l as f32;
        }

        PhaseBase(base)
    }

    /// Retrieve the phase term Psi_l, 1 <= l <= 56.
    pub(crate) fn get(&self, l: usize) -> f32 {
        self.0[l - 1]
    }
}

impl Default for PhaseBase {
    /// Create a new `PhaseBase` in the default state.
    ///
    /// By default all phase terms are 0 [p64].
    fn default() -> Self {
        PhaseBase([0.0; MAX_HARMONICS])
    }
}

/// Random phase terms Phi_l derived from the base phase offsets.
///
/// For harmonics above L/4, adds a random perturbation scaled by the
/// fraction of unvoiced harmonics (Eq 140).
pub(crate) struct Phase([f32; MAX_HARMONICS]);

impl Phase {
    /// Create a new `Phase` building on the given base phase terms.
    ///
    /// Copies base phase offsets, then for harmonics from L/4 + 1 through
    /// max(L, L_prev), adds a random perturbation scaled by the unvoiced
    /// fraction (Eq 140).
    pub(crate) fn new<R: Rng>(
        base: &PhaseBase,
        params: &BaseParams,
        prev: &PrevFrame,
        voice: &VoiceDecisions,
        mut noise: R,
    ) -> Self {
        let mut phase = base.0;

        // Compute bounds for modification used in Eq 140.
        let start = params.harmonic_count as usize / 4;
        let stop = max(params.harmonic_count, prev.params.harmonic_count) as usize;

        // Compute common scaling factor in Eq 140.
        let scale = voice.unvoiced_count() as f32 / params.harmonic_count as f32;

        // Modify Psi_l from start + 1 <= l <= stop.
        // Since i = l - 1, start <= i <= stop - 1.
        for p in &mut phase[start..stop] {
            *p += scale * noise.random_range(-PI..PI);
        }

        Phase(phase)
    }

    /// Retrieve the phase term Phi_l, 1 <= l <= 56.
    pub(crate) fn get(&self, l: usize) -> f32 {
        self.0[l - 1]
    }
}

impl Default for Phase {
    /// Create a new `Phase` in the default state.
    ///
    /// By default all phase terms are 0 [p64].
    fn default() -> Self {
        Phase([0.0; MAX_HARMONICS])
    }
}

/// Synthesizes voiced spectrum signal s_v(n).
///
/// Combines current and previous frame contributions using a four-way
/// blending scheme based on current and previous voiced/unvoiced decisions
/// (Eqs 127, 130-133).
pub(crate) struct Voiced<'a> {
    /// Previous frame parameters.
    prev: &'a PrevFrame,
    /// Phase terms for the current frame.
    phase: &'a Phase,
    /// Enhanced spectral amplitudes for the current frame.
    enhanced: &'a EnhancedSpectrals,
    /// Voiced/unvoiced decisions for the current frame.
    voice: &'a VoiceDecisions,
    /// Synthesis window for combining voiced/unvoiced frames.
    window: window::Window,
    /// Fundamental frequency of the current frame.
    fundamental_frequency: f32,
    /// Upper bound of summation: max(L, L_prev).
    sum_bound: usize,
}

impl<'a> Voiced<'a> {
    /// Create a new `Voiced` synthesizer from the given frame parameters.
    pub(crate) fn new(
        params: &BaseParams,
        prev: &'a PrevFrame,
        phase: &'a Phase,
        enhanced: &'a EnhancedSpectrals,
        voice: &'a VoiceDecisions,
    ) -> Self {
        Voiced {
            prev,
            phase,
            enhanced,
            voice,
            window: window::synthesis(),
            fundamental_frequency: params.fundamental_frequency,
            // Compute the sum bound in Eq 127.
            sum_bound: max(params.harmonic_count, prev.params.harmonic_count) as usize,
        }
    }

    /// Compute s_v,l(n), the signal level at sample n for the l-th spectral
    /// amplitude using four-way blending (Eqs 130-133).
    fn get_pair(&self, l: usize, n: isize) -> f32 {
        match (self.voice.is_voiced(l), self.prev.voice.is_voiced(l)) {
            // Eq 130: unvoiced -> unvoiced.
            (false, false) => 0.0,
            // Eq 131: unvoiced current, voiced previous.
            (false, true) => self.signal_previous(l, n),
            // Eq 132: voiced current, unvoiced previous.
            (true, false) => self.signal_current(l, n),
            // Eq 133: voiced -> voiced.
            (true, true) => self.signal_previous(l, n) + self.signal_current(l, n),
        }
    }

    /// Compute s_v,l(n) for a voiced current frame and unvoiced previous frame (Eq 132).
    fn signal_current(&self, l: usize, n: isize) -> f32 {
        self.window.get(n - SAMPLES_PER_FRAME as isize)
            * self.enhanced.get(l)
            * (self.fundamental_frequency * (n - SAMPLES_PER_FRAME as isize) as f32 * l as f32
                + self.phase.get(l))
            .cos()
    }

    /// Compute s_v,l(n) for an unvoiced current frame and voiced previous frame (Eq 131).
    fn signal_previous(&self, l: usize, n: isize) -> f32 {
        self.window.get(n)
            * self.prev.enhanced.get(l)
            * (self.prev.params.fundamental_frequency * n as f32 * l as f32
                + self.prev.phase.get(l))
            .cos()
    }

    /// Compute the voiced signal sample s_v(n) for the given sample n,
    /// 0 <= n < 160 (Eq 127).
    pub(crate) fn get(&self, n: usize) -> f32 {
        debug_assert!(n < SAMPLES_PER_FRAME);

        // Compute Eq 127.
        2.0 * (1..=self.sum_bound)
            .map(|l| self.get_pair(l, n as isize))
            .fold(0.0, |s, x| s + x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::descramble::{Bootstrap, descramble};
    use rand::SeedableRng;
    use rand::rngs::SmallRng;

    #[test]
    fn phase_base_accumulation() {
        // Verify results match standalone python script.
        let chunks = [
            0b001000010010,
            0b110011001100,
            0b111000111000,
            0b111111111111,
            0b10101110101,
            0b00101111010,
            0b01110111011,
            0b00001000,
        ];

        let bootstrap = Bootstrap::new(&chunks);
        let params = BaseParams::new(bootstrap.unwrap_period());
        let mut prev = PrevFrame::default();

        for l in 1..=56 {
            prev.phase_base.0[l - 1] = l as f32;
        }

        assert!((params.fundamental_frequency - 0.17575344).abs() < 0.000001);
        assert!((prev.params.fundamental_frequency - 0.02985 * PI).abs() < 0.000001);
        assert_eq!(params.harmonic_count, 16);

        let pb = PhaseBase::new(&params, &prev);

        assert!((pb.get(1) - 22.56239845600000037961763155180961).abs() < 1e-3);
        assert!((pb.get(2) - 45.12479691200000075923526310361922).abs() < 1e-3);
        assert!((pb.get(3) - 67.68719536800000469156657345592976).abs() < 1e-3);
        assert!((pb.get(4) - 90.24959382400000151847052620723844).abs() < 1e-3);
        assert!((pb.get(5) - 112.81199227999999834537447895854712).abs() < 1e-3);
        assert!((pb.get(6) - 135.37439073600000938313314691185951).abs() < 1e-3);
        assert!((pb.get(7) - 157.93678919199999199918238446116447).abs() < 1e-3);
        assert!((pb.get(8) - 180.49918764800000303694105241447687).abs() < 1e-3);
        assert!((pb.get(9) - 203.06158610399998565299028996378183).abs() < 1e-3);
        assert!((pb.get(10) - 225.62398455999999669074895791709423).abs() < 1e-3);
        assert!((pb.get(11) - 248.18638301600003615021705627441406).abs() < 1e-3);
        assert!((pb.get(12) - 270.74878147200001876626629382371902).abs() < 1e-3);
        assert!((pb.get(13) - 293.31117992800000138231553137302399).abs() < 1e-3);
        assert!((pb.get(14) - 315.87357838399998399836476892232895).abs() < 1e-3);
        assert!((pb.get(15) - 338.43597684000002345783286727964878).abs() < 1e-3);
        assert!((pb.get(16) - 360.99837529600000607388210482895374).abs() < 1e-3);
        assert!((pb.get(17) - 383.56077375199998868993134237825871).abs() < 1e-3);
        assert!((pb.get(18) - 406.12317220799997130598057992756367).abs() < 1e-3);
        assert!((pb.get(19) - 428.68557066400001076544867828488350).abs() < 1e-3);
        assert!((pb.get(20) - 451.24796911999999338149791583418846).abs() < 1e-3);
        assert!((pb.get(21) - 473.81036757599997599754715338349342).abs() < 1e-3);
        assert!((pb.get(22) - 496.37276603200007230043411254882812).abs() < 1e-3);
        assert!((pb.get(23) - 518.93516448800005491648335009813309).abs() < 1e-3);
        assert!((pb.get(24) - 541.49756294400003753253258764743805).abs() < 1e-3);
        assert!((pb.get(25) - 564.05996140000002014858182519674301).abs() < 1e-3);
        assert!((pb.get(26) - 586.62235985600000276463106274604797).abs() < 1e-3);
        assert!((pb.get(27) - 609.18475831199998538068030029535294).abs() < 1e-3);
        assert!((pb.get(28) - 631.74715676799996799672953784465790).abs() < 1e-3);
        assert!((pb.get(29) - 654.30955522399995061277877539396286).abs() < 1e-3);
        assert!((pb.get(30) - 676.87195368000004691566573455929756).abs() < 1e-3);
        assert!((pb.get(31) - 699.43435213600002953171497210860252).abs() < 1e-3);
        assert!((pb.get(32) - 721.99675059200001214776420965790749).abs() < 1e-3);
        assert!((pb.get(33) - 744.55914904799999476381344720721245).abs() < 1e-3);
        assert!((pb.get(34) - 767.12154750399997737986268475651741).abs() < 1e-3);
        assert!((pb.get(35) - 789.68394595999995999591192230582237).abs() < 1e-3);
        assert!((pb.get(36) - 812.24634441599994261196115985512733).abs() < 1e-3);
        assert!((pb.get(37) - 834.80874287200003891484811902046204).abs() < 1e-3);
        assert!((pb.get(38) - 857.37114132800002153089735656976700).abs() < 1e-3);
        assert!((pb.get(39) - 879.93353978400000414694659411907196).abs() < 1e-3);
        assert!((pb.get(40) - 902.49593823999998676299583166837692).abs() < 1e-3);
        assert!((pb.get(41) - 925.05833669599996937904506921768188).abs() < 1e-3);
        assert!((pb.get(42) - 947.62073515199995199509430676698685).abs() < 1e-3);
        assert!((pb.get(43) - 970.18313360799993461114354431629181).abs() < 1e-3);
        assert!((pb.get(44) - 992.74553206400014460086822509765625).abs() < 1e-3);
        assert!((pb.get(45) - 1015.30793052000012721691746264696121).abs() < 1e-3);
        assert!((pb.get(46) - 1037.87032897600010983296670019626617).abs() < 1e-3);
        assert!((pb.get(47) - 1060.43272743200009244901593774557114).abs() < 1e-3);
        assert!((pb.get(48) - 1082.99512588800007506506517529487610).abs() < 1e-3);
        assert!((pb.get(49) - 1105.55752434400005768111441284418106).abs() < 1e-3);
        assert!((pb.get(50) - 1128.11992280000004029716365039348602).abs() < 1e-3);
        assert!((pb.get(51) - 1150.68232125600002291321288794279099).abs() < 1e-3);
        assert!((pb.get(52) - 1173.24471971200000552926212549209595).abs() < 1e-3);
        assert!((pb.get(53) - 1195.80711816799998814531136304140091).abs() < 1e-3);
        assert!((pb.get(54) - 1218.36951662399997076136060059070587).abs() < 1e-3);
        assert!((pb.get(55) - 1240.93191507999995337740983814001083).abs() < 1e-3);
        assert!((pb.get(56) - 1263.49431353599993599345907568931580).abs() < 1e-3);
    }

    #[test]
    fn phase_randomization() {
        // Verify results match standalone python script.
        let chunks = [
            0b001000010010,
            0b110011001100,
            0b111000111000,
            0b111111111111,
            0b10101110101,
            0b00101111010,
            0b01110111011,
            0b00001000,
        ];

        let bootstrap = Bootstrap::new(&chunks);
        let params = BaseParams::new(bootstrap.unwrap_period());
        let (_, voice, _) = descramble(&chunks, &params).unwrap();
        let mut prev = PrevFrame::default();

        for l in 1..=56 {
            prev.phase_base.0[l - 1] = l as f32;
        }

        assert_eq!(params.harmonic_count, 16);
        assert_eq!(prev.params.harmonic_count, 30);
        assert_eq!(voice.unvoiced_count(), 6);

        let pb = PhaseBase::new(&params, &prev);

        // Use a seeded SmallRng for reproducibility.
        let rng = SmallRng::seed_from_u64(0);
        let phase = Phase::new(&pb, &params, &prev, &voice, rng);

        // First 4 harmonics (l=1..4) are unmodified (below L/4 = 4 boundary).
        assert!((phase.get(1) - pb.get(1)).abs() < 1e-6);
        assert!((phase.get(2) - pb.get(2)).abs() < 1e-6);
        assert!((phase.get(3) - pb.get(3)).abs() < 1e-6);
        assert!((phase.get(4) - pb.get(4)).abs() < 1e-6);

        // Harmonics 5 through 30 (max(16, 30) = 30) should be modified.
        let mut any_modified = false;
        for l in 5..=30 {
            if (phase.get(l) - pb.get(l)).abs() > 1e-6 {
                any_modified = true;
            }
        }
        assert!(any_modified, "expected some phase values to be modified");

        // Harmonics above 30 should be unmodified.
        for l in 31..=56 {
            assert!(
                (phase.get(l) - pb.get(l)).abs() < 1e-6,
                "harmonic {l} should not be modified"
            );
        }
    }

    #[test]
    fn phase_base_default_is_zero() {
        let pb = PhaseBase::default();
        for l in 1..=56 {
            assert_eq!(pb.get(l), 0.0);
        }
    }

    #[test]
    fn phase_default_is_zero() {
        let phase = Phase::default();
        for l in 1..=56 {
            assert_eq!(phase.get(l), 0.0);
        }
    }

    #[test]
    fn voiced_silent_when_both_unvoiced() {
        // When both current and previous harmonics are unvoiced,
        // all voiced signal samples should be zero (Eq 130).
        let prev = PrevFrame::default();
        let params = BaseParams::new(32); // L=16
        let phase = Phase::default();
        let enhanced = EnhancedSpectrals::default();

        // All-unvoiced voice decisions.
        let voice = VoiceDecisions::new(0, &params);

        let voiced = Voiced::new(&params, &prev, &phase, &enhanced, &voice);

        for n in 0..SAMPLES_PER_FRAME {
            assert_eq!(voiced.get(n), 0.0, "expected zero at sample {n}");
        }
    }
}
