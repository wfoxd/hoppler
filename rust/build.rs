fn main() {
    prost_build::compile_protos(
        &[
            "../proto/hello.proto",
            "../proto/envelope.proto",
            "../proto/identity.proto",
            "../proto/pairing.proto",
        ],
        &["../proto"],
    )
    .expect("protobuf compilation failed — is protoc installed?");
    println!("cargo:rerun-if-changed=../proto");
}
