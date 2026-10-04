# Upstream ve Kaynak Bileşenler

TOY Office'in ilk geliştirme tabanı Euro-Office Lite projesidir.

Temel mimari:

- Tauri v2 masaüstü kabuğu
- Rust backend
- Euro-Office sdkjs editör motoru
- Euro-Office web-apps kullanıcı arayüzü
- x2t belge dönüştürücü

## Geliştirme ilkesi

sdkjs ve web-apps mümkün olduğunca değiştirilmeden alt modül olarak tutulacaktır. TOY Office'e özgü kod öncelikle Tauri/Rust, bridge, paketleme, yerelleştirme ve marka katmanlarında geliştirilecektir.

Upstream lisans, telif ve kaynak bildirimleri korunacaktır. Dağıtım öncesinde AGPL-3.0 ve ilgili ek bildirimler ayrıca denetlenecektir.
