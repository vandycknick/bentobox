fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/common.proto");
    println!("cargo:rerun-if-changed=proto/agent.proto");
    println!("cargo:rerun-if-changed=proto/instance_control.proto");

    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure().compile_protos(
        &[
            "proto/common.proto",
            "proto/agent.proto",
            "proto/instance_control.proto",
        ],
        &["proto"],
    )?;

    Ok(())
}
