# Re:Kord (リコード)

PC で再生中の音声(WASAPI ループバック)から、コードとノートをリアルタイムに認識するツールです。
認識結果はピアノロールとコード名で表示され、キー(調)とローマ数字表記、メロディ/ベースの判別も行います。

## 仕組み

1. **ノート推定** — Spotify の [basic-pitch](https://github.com/spotify/basic-pitch)(ONNX)でポリフォニックにピッチを推定します。
2. **コード判定** — 音高クラス集合とコードテンプレート(テンションコード約 300 種)のルールベース照合で、コード名を決めます。
3. **キー推定** — Krumhansl-Schmuckler 系のピッチクラス相関(Bellman-Budge プロファイル)で調を推定し、コードをローマ数字で表します。
4. **メロディ/ベース** — 確定したノート列に対する記号的なスカイライン規則(最高音/最低音)で判別します。

コード名とキーに機械学習モデルは使っていません。設計の理由は [CLAUDE.md](CLAUDE.md) にあります。

## 動作環境

- Windows 11(音声取得に WASAPI ループバックを使うため Windows 専用です)
- ONNX Runtime の DirectML 実行プロバイダが使える GPU(RTX 5070 Ti で動作確認)
- [Rust](https://rustup.rs/) と [Node.js](https://nodejs.org/)

## 使い方

### デスクトップアプリ(ピアノロール UI)

```powershell
Set-Location rekord-piano-roll
npm install
npm run tauri dev
```

起動するとすぐにキャプチャを始めます。システムで再生されている音声がそのまま解析対象です。
ライブ入力専用で、ファイルを読み込む UI はありません。

### CLI

```bash
cd rust
cargo build --release
./target/release/rekord_engine.exe ../sample_audio/piano_progression_C_Am_F_G.wav
./target/release/rekord_engine.exe --live
```

| オプション | 内容 |
| --- | --- |
| `--live` | 既定のスピーカー出力をループバックで取得して解析します |
| `--duration <秒>` | 指定秒数で終了します(`--live` 用) |
| `--latency <秒>` | 確定までの遅延を指定します(モデルの窓の進め幅に影響します) |
| `--hop <秒>` | 取得データの受け渡し間隔です |

初回ビルド時に basic-pitch のモデル(`nmp.onnx`)を GitHub から `rust/core/models/` にダウンロードします。

## 構成

```
rust/
  core/                rekord_core(推論・ノート抽出・コード/キー判定・キャプチャ)
  cli/                 rekord_engine(CLI)
rekord-piano-roll/     Tauri + Svelte 5 + Pixi.js のピアノロール UI
sample_audio/          検証用のピアノ録音
```

## 制限

- キー推定は直近の数秒〜十数秒の窓で行うため、主和音がまだ鳴っていない区間では外れることがあります。
- ドラムなど非ピアノの音が多い音源では、誤検出が増えます。
- 今後の予定は [TODO.md](TODO.md) にあります。

## サードパーティ

- [basic-pitch](https://github.com/spotify/basic-pitch) のモデル(Apache License 2.0)は、リポジトリには含めず、ビルド時にダウンロードします。
- 主な依存: [ort](https://github.com/pykeio/ort)、[cpal](https://github.com/RustAudio/cpal)、[Tauri](https://tauri.app/)、[Svelte](https://svelte.dev/)、[Bits UI](https://bits-ui.com/)、[Pixi.js](https://pixijs.com/)

## ライセンス

[MIT License](LICENSE)

`sample_audio/` の音源はすべて作者自身が制作したもので、同じライセンスで提供します。
