//! Physical-unit calibration for *display only*. The simulation runs in arbitrary
//! units (`G = 1`); these factors map them to physical galaxy scales for the
//! on-screen rotation curve and clock. The solver never sees them — changing them
//! re-labels the visuals without touching the dynamics.
//!
//! Two anchors fix the system: one length unit = 0.1 kpc, and the default halo's
//! asymptotic circular speed (`HALO_V0`) = 220 km/s (a Milky-Way-like flat curve).
//! Because `G = 1` in the sim, the time scale then follows from length / velocity
//! (1 kpc per km/s ≈ 978.5 Myr), so the same equations hold in physical units. With
//! these, the disk scale length (`DISK_RD = 35`) reads ~3.5 kpc and the stellar mass
//! (~`NUM_PARTICLES · STAR_MASS`, disk + bulge) ~7×10¹⁰ M☉ — all galaxy-plausible.

use crate::simulation::HALO_V0;

/// Kiloparsecs per simulation length unit.
pub const KPC_PER_UNIT: f32 = 0.1;

/// km/s per simulation velocity unit — anchored so the default halo flattens at
/// 220 km/s.
pub const KMS_PER_UNIT: f32 = 220.0 / HALO_V0;

/// Megayears per simulation time unit: `(kpc / (km/s)) · 978.46 Myr`.
pub const MYR_PER_UNIT: f32 = (KPC_PER_UNIT / KMS_PER_UNIT) * 978.46;
