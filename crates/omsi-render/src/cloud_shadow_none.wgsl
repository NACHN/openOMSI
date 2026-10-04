// Stands in for cloud_shadow.wgsl on a device that cannot hold the shape map as a
// seventeenth sampled texture of the fragment stage (lib.rs, `cloud_shadow`): the sun
// reaches every surface, as it did before the clouds cast anything.
fn cloud_shadow(world: vec3<f32>) -> f32 {
    return 1.0;
}
