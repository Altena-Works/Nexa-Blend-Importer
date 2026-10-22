# Nexa Blend Importer

[日本語](#日本語) | [English](#english)

## 日本語

Nexa Blend Importerは、MMDの全バイナリのリバースエンジニアリングと改良を目指すNexa計画から派生した、Blenderのモデル・アニメーション・カメラをMMDへ取り込むための独立プロジェクトです。

v0.3.0では、モデルPMX・モデルモーションVMD・カメラVMDの変換と3要素GUIを実装しています。対応範囲について、7通りの選択・取消・既存ファイル保護・日英表示・MMD実描画の検証を完了しました。任意のblendへの対応を保証するものではありません。[検証状況](VERIFICATION.md)を参照してください。

### 配置と使い方

配布候補ZIPの `NexaBlendImporter` フォルダーを、MMDPluginを導入したMMDの `Plugin` フォルダーへ配置します。MMD・MMDPlugin・Blenderは同梱しません。Windows x64向けで、ビルド済みパッケージを使う場合はRust不要です。正式Releaseはまだありません。

今回のビルドはCRTを静的リンクしており、このプラグインのためのVC++ランタイム追加導入は不要です。MMDやMMDPlugin自身の動作要件は別途満たしてください。既存環境へ配置する前にMMDを終了し、旧プラグインフォルダーをバックアップしてください。元に戻す場合はMMDを終了してバックアップを復元します。

1. MMDの「Nexa Blend Importer → blendを取り込み」から `.blend` を選びます。
2. モデル・カメラ・アニメーションを選び、検出されたBlenderを確認して「取り込み」を押します。詳細ではフレーム範囲・倍率・出力先などを指定できます。
3. 結果の新規フォルダーから、PMX→対応するモデルモーションVMD→カメラVMDの順でMMDへ標準読込みします。カメラVMDはカメラ・照明モードで読み込みます。先頭に配置する場合は現在フレームを0に戻してください。

モデルを選ばずアニメーションだけを取り込む場合は、各モデルに対応する既存PMXを指定します。名前・親・基準姿勢・モーフなどを照合し、不一致なら出力しません。カメラを選びアニメーションを外すと、基準フレームの静止カメラを出力します。入力blendを保存し直さず、既存出力を上書きしません。モデルやカメラの自動読込み・ライブ同期はありません。

材質は拡散色・アルファ・画像1枚へ変換し、PBRシェーダーの見た目は再現しません。未対応のmodifier・変形・材質設定などが選択範囲に含まれる場合は理由を表示して中止します。非整数のカメラ視野角は、詳細で近似を許可した場合だけ変換します。

v0.3.0はプレビュー版です。以前の試験でMMDが応答しなくなった事象の原因は未特定です。現候補の新規配置・変換・再起動・終了試験は通っていますが、長時間運用とすべてのWindows環境は未検証です。重要なPMMは保存・バックアップしてから試してください。パッケージは未署名です。

### 開発環境

Rust 1.95.0、MSVC x64、Windows。Blender、MMD、MMDPluginは別途必要です。カメラ評価・変換には、固定したNexa Blend Cameraの自作ソースをこのworkspace内で再利用します。Nexa solverや別repoへのローカルpath依存はありません。

```powershell
cargo test --offline --locked -p nexa-blend-importer
cargo build --offline --locked --release --workspace
```

実blendのCLI変換は `nexa-blend-importer.exe --import scene.blend --selection model,camera,animation --output-parent D:\Output` を使います。Blenderを検出できない場合は `--blender` で実行ファイルを指定します。アニメーションのみではモデルごとに `--target-pmx` を追加します。詳細は `--help` を参照してください。

自作部分は[Apache-2.0](LICENSE)です。[第三者の通知](THIRD_PARTY_NOTICES.md)を保持しています。v0.3.0は開発中の版番号で、公開Releaseはありません。

### 謝辞

MikuMikuDance（MMD）の作者である樋口優氏をはじめ、MMDPlugin、MMEffect、Blender、Bullet、Saba、GLMの開発者・貢献者、モデル・モーション・エフェクト・資料の制作者、MMDコミュニティの皆様に感謝します。

## English

Nexa Blend Importer is an independent outcome of Nexa, a project for reverse-engineering all MMD binaries and improving the resulting implementation. It aims to import Blender models, animation and cameras into MMD.

Version 0.3.0 implements model PMX, model-motion VMD, camera VMD conversion and the component-selection GUI. Bounded checks of all seven selections, cancellation, existing-file protection, Japanese/English UI and native MMD rendering pass. This does not guarantee support for arbitrary blend files. See [verification status](VERIFICATION.md).

### Installation and use

Place the candidate ZIP's `NexaBlendImporter` folder inside the `Plugin` folder of an MMD installation with MMDPlugin. MMD, MMDPlugin and Blender are separate requirements and are not bundled. The package targets Windows x64 and does not require Rust. No formal release is available yet.

The CRT is linked statically, so this plugin does not require a separate VC++ runtime installation. MMD and MMDPlugin may have their own prerequisites. Close MMD and back up the previous plugin folder before replacing it. To roll back, close MMD and restore that backup.

1. Choose **Nexa Blend Importer → Import .blend...** in MMD, then select a `.blend` file.
2. Select **Model**, **Camera** and/or **Animation**, confirm the detected Blender executable, then choose **Import**. **Details** provides frame range, unit scale, output directory and other settings.
3. Load the generated PMX, its model-motion VMD, then the camera VMD through MMD's standard operations. Use camera/light mode for the camera VMD. Set the current frame to 0 to load from the beginning.

Animation-only import asks for an existing PMX for each model and verifies names, parents, rest positions and morph correspondence. A mismatch stops publication. Camera without Animation exports a static reference-frame camera. The input blend is not saved and existing outputs are not overwritten. Automatic MMD loading and live synchronization are not included.

Materials convert to diffuse color, alpha and one image; PBR shader appearance is not reproduced. Unsupported selected modifiers, deformation or material settings stop the import with a reason. Non-integer camera FOV conversion requires explicit approximation permission in Details.

Version 0.3.0 is a preview. A previously observed MMD hang has no confirmed cause. The current candidate passes fresh installation, conversion, relaunch and exit checks, but long sessions and every Windows environment are unverified. Save and back up important PMM projects before trying it. The package is unsigned.

### Development

Use Rust 1.95.0 and the MSVC x64 build tools on Windows. Blender, MMD and MMDPlugin are separate requirements. A pinned snapshot of original Nexa Blend Camera source is reused within this workspace, with no local path dependency on another repository or the Nexa solver. See the commands above.

For CLI conversion, use `nexa-blend-importer.exe --import scene.blend --selection model,camera,animation --output-parent D:\Output`. Use `--blender` if discovery finds no executable. Animation-only import takes one `--target-pmx` per model. See `--help` for details.

Original code is licensed under [Apache-2.0](LICENSE). See [third-party notices](THIRD_PARTY_NOTICES.md). Version 0.3.0 is under development; no release has been published.

### Acknowledgements

We thank MikuMikuDance (MMD) creator 樋口優, the developers and contributors of MMDPlugin, MMEffect, Blender, Bullet, Saba and GLM, asset and reference-material creators, and the MMD community.
