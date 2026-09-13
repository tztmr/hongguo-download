// Keep the entire displayed frame (including anamorphic pixels and FFmpeg's
// automatic rotation) inside a square. Avoid upscaling ordinary small sources.
pub(super) fn square_side(width: u32, height: u32) -> u32 {
    width.max(height).clamp(2, 1080) & !1
}

pub(super) fn square_filter(side: u32) -> String {
    format!("scale=w='if(gte(dar,1),{side},max(2,trunc({side}*dar/2)*2))':h='if(gte(dar,1),max(2,trunc({side}/dar/2)*2),{side})',setsar=1,pad={side}:{side}:(ow-iw)/2:(oh-ih)/2:color=black")
}
