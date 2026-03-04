fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/instance/v1/instance_control.proto");

    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .compile_protos(&["proto/instance/v1/instance_control.proto"], &["proto"])?;

    Ok(())
}
