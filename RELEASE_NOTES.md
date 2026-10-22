# Nexa Blend Importer v0.3.0 — preview

blendのモデル・アニメーション・カメラを選択し、MMD用PMX/VMDへ変換するプレビュー版です。公開用候補をローカルで準備済みで、GitHub Releaseはまだ公開していません。

## インストール

`Nexa-Blend-Importer-0.3.0-x64.zip` はWindows x64向けのビルド済みパッケージです。展開した `NexaBlendImporter` を、MMDPlugin導入済みMMDの `Plugin` に配置します。Rustの導入・ビルドは不要です。配布時はZIPと同名の `.zip.sha256` を添付します。

MMD・MMDPlugin・Blenderは同梱しません。既存版を置き換える際はMMDを終了し、旧フォルダーをバックアップしてください。生成物はPMX→対応するモデルモーションVMD→カメラVMDの順に手動で読み込みます。

## 変更点と確認

- 日英UI、Blender自動検出、3要素の選択、取消、既存出力保護。
- GUIのmodal-loop再入を防止。CRT静的リンクにより、このプラグインのVC++ランタイム追加導入は不要。
- 単体試験45件と、新規ZIP配置からの実MMD起動・変換・worker再起動・正常終了を確認。

## 制限

以前のMMDハングの原因は未特定です。現在の候補の限定試験では再現しませんでしたが、長時間運用・任意blend・全Windows環境は未検証です。重要なPMMは事前に保存・バックアップしてください。パッケージは未署名です。対応範囲と詳細は [README](README.md) と [検証状況](VERIFICATION.md) を参照してください。

## English

This preview imports selected Blender models, animation and cameras as PMX/VMD files. The Windows x64 ZIP is prebuilt; Rust and a separate VC++ runtime for this plugin are not required. MMD, MMDPlugin and Blender remain external prerequisites. Close MMD and back up the old plugin folder before installation. Load generated PMX, model-motion VMD and camera VMD manually in that order.

The candidate passes 45 unit tests and fresh-install MMD startup, conversion, worker relaunch and normal exit. It is unsigned. A previously reported MMD hang has no confirmed cause; long sessions and arbitrary scenes remain unverified. Save important projects before use. No GitHub release has been published yet.
