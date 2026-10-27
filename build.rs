fn main() {
    // Embeds the app icon (resource id 1) used by Explorer and the tray.
    embed_resource::compile("assets/blazecursor.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
