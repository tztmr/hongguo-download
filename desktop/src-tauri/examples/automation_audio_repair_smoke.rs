//! Local-only regression using real episodes with different AAC decoder configurations.
use hongguo_desktop_lib::media::{
    self, model::MergeMode, CancellationToken, MediaTools, StartMergeInput, StartMergeRequest,
};
use std::{fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).ok_or("new output directory required")?);
    fs::create_dir(&root)?;
    let mut inputs = vec![];
    for (i, source) in args[2..].iter().enumerate() {
        let path = root.join(format!("{:04}.mp4", i + 1));
        fs::copy(source, &path)?;
        inputs.push(StartMergeInput {
            episode_index: i as u32 + 1,
            path,
        });
    }
    let request = media::validate_merge_request(&StartMergeRequest {
        book_id: "auto-audio-repair-proof".into(),
        title: "真实音轨兼容验证".into(),
        series_root: root.clone(),
        output_file_name: "验证成片.mp4".into(),
        inputs,
        transcode_h264: false,
        square_canvas: false,
        mode: Some(MergeMode::Auto),
        quality: Default::default(),
        conflict_policy: Default::default(),
    })?;
    let exe = std::env::current_exe()?;
    let tools = MediaTools::from_resource_root(exe.parent().unwrap())?;
    let result = media::run_merge(
        &tools,
        request.execution_request(),
        &CancellationToken::default(),
        |p| println!("{} {:.0}%", p.stage, p.percent),
    )?;
    let output = std::process::Command::new(tools.ffmpeg())
        .args(["-v", "error", "-xerror", "-i"])
        .arg(&result.output_path)
        .args(["-map", "0:a:0", "-f", "null", "-"])
        .output()?;
    fs::write(root.join("decode-diagnostic.txt"), &output.stderr)?;
    assert!(
        output.status.success(),
        "merged audio must decode without errors"
    );
    println!("verified {}", result.output_path.display());
    Ok(())
}
