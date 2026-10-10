# TODO

上ほど優先度が高い。

- ピアノロールの選択範囲を .mid としてドラッグアウトする処理を自前で実装する(現状は `drag` crate)。ドラッグ中に Windows 標準の点線枠のほかに、正体不明の画像が右下に出る(Explorer からの .mid のドラッグでは出ない)。`drag` crate に渡す画像を変えても(アプリアイコン、専用サムネイル、画像なし)見た目は変わらず、crate 側では直せない。`DoDragDrop` と `IDragSourceHelper` を直接呼んで、ドラッグ画像を自分で指定する。動作自体(Bitwig・Studio One へのドロップ)は問題ない。実装は `rekord-piano-roll/src-tauri/src/midi_copy.rs` の `drag`
