//! Head pose and mood, shared by the faces and the mannequin.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Booting,
    Idle,
    Listening,
    Thinking,
    Speaking,
    Error,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// 0 closed .. 1 wide open.
    pub mouth: f32,
    /// Brow lift, 0..1.
    pub brow: f32,
    /// Head offset in render-target pixels.
    pub dx: f32,
    pub dy: f32,
}
