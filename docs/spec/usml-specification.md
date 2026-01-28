# USML Specification v0.1

> **Usecase Markup Language** — OpenAPI と DBML の間を埋め、APIユースケースレベルのデータフローを声明的に定義する言語。

---

## 1. 目的と動機

### 現在の問題

| ツール | 強み | ギャップ |
|---|---|---|
| OpenAPI | エンドポイント・リクエスト・レスポンス構造 | データの源泉（テーブル・カラム・結合条件）が不明 |
| DBML | テーブル・カラム・FK関係 | APIとの対応・クエリ意図・変換ロジックが不明 |

「ユーザー一覧取得API」のレスポンスに `avatar_url` がある場合、OpenAPI では「文字列フィールド」としか定義できない。実際には `profiles` テーブルの `avatar_url` カラムで、`users.id = profiles.user_id` で結合されていることは別のドキュメントを見なければわからない。

### USML の役割

```
OpenAPI ──┐
          ├──► USML ──► 視覚化・検証
DBML ─────┘
```

USML はOpenAPIやDBMLを**import**し、「1つのAPIリクエストがDBのどのデータを、どのように組み立てて返すか」を声明的に記述する。

---

## 2. ファイル構造

USML ファイルは YAML を基にする。拡張子は `.usml.yaml` とする。

```yaml
# 基本テンプレート
version: "0.1"

import:
  openapi: <パス>#<参照先>
  dbml:
    - <パス>#tables["<テーブル名>"]

usecase:
  name: <ユースケース名>
  summary: <説明>

  response_mapping:
    - <マッピング定義>

  filters:
    - <フィルタ定義>

  transforms:
    - <変換定義>
```

---

## 3. Import セクション

外部仕様ファイルへの参照を定義する。

### 3.1 OpenAPI Import

```yaml
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
```

- パス部分は USML 独自の参照記法を使用する
- `paths["<path>"].<method>` で特定エンドポイント・メソッドを指定
- `.responses["<ステータスコード>"]` でレスポンスコードを明示する。省略時は `"200"` がデフォルトとなる
- 参照されたレスポンススキーマが `response_mapping` の検証元になる

> **参照記法の文法**
> ```
> <ファイルパス>#paths["<パス>"].<メソッド>.responses["<ステータスコード>"]
> ```
> 例: `./api.yaml#paths["/posts/{post_id}"].get.responses["200"]`

### 3.2 DBML Import

```yaml
import:
  dbml:
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["profiles"]
```

- `tables["<名前>"]` でテーブル単位で参照
- `tables["<名前>"].columns["<名前>"]` でカラム単位も可
- 参照されたテーブルが `response_mapping` の結合・ソース元になる

---

## 4. response_mapping セクション

APIレスポンスの各フィールドとDBカラムの対応を定義する。これがUSMLの核心である。

### 4.1 単純対応（1テーブル・1カラム）

```yaml
response_mapping:
  - field: id
    source: users.id
  - field: name
    source: users.name
```

- `field`: レスポンスのフィールド名（OpenAPIのレスポンススキーマと照合）
- `source`: `<テーブル名>.<カラム名>` 形式

### 4.2 結合あり

別テーブルのカラムを使うときは `join` を添付する。

```yaml
response_mapping:
  - field: avatar_url
    source: profiles.avatar_url
    join:
      table: profiles
      on: users.id = profiles.user_id
      type: LEFT JOIN  # デフォルトは LEFT JOIN
```

- `on`: 結合条件（式として記述）
- `type`: `INNER JOIN` / `LEFT JOIN` / `RIGHT JOIN`（デフォルト: `LEFT JOIN`）
- `alias`: テーブルのエイリアス名。同じテーブルを異なる結合条件で複数回参照する場合に必要
- 同じ `join.table`（かつエイリアス無し）が複数マッピングに出る場合は、最初の定義で統一される
- 異なる `on` 条件で同テーブルを参照する場合は、必ず `alias` を指定し、`source` でもエイリアス名を使用する

エイリアスの使用例：

```yaml
response_mapping:
  # 投稿著者
  - field: author_name
    source: post_author.name
    join:
      table: users
      alias: post_author
      on: posts.user_id = post_author.id
  # コメント著者（同じusersテーブルだが別の結合条件）
  - field: last_comment_author
    source: comment_author.name
    join:
      table: users
      alias: comment_author
      on: posts.last_comment_user_id = comment_author.id
```

### 4.3 集約参照

1対多の関係で集約が必要なケース。

```yaml
response_mapping:
  - field: comment_count
    source: comments.id
    join:
      table: comments
      on: posts.id = comments.post_id
    aggregate:
      type: COUNT
      group_by: posts.id
```

- `aggregate.type`: `COUNT` / `SUM` / `AVG` / `MIN` / `MAX`
- `aggregate.group_by`: 集約の GROUP BY キーを明示する。省略時はルートテーブルの主キーを自動で適用する
- `aggregate` とJOINは組み合わせ可能

### 4.4 配列フィールド

ネストされた配列レスポンスの場合。

```yaml
response_mapping:
  - field: comments
    type: array
    source_table: comments
    join:
      table: comments
      on: posts.id = comments.post_id
    fields:
      - field: id
        source: comments.id
      - field: body
        source: comments.body
      - field: author_name
        source: users.name
        join:
          table: users
          on: comments.user_id = users.id
```

- `type: array` で配列レスポンスを示す
- `source_table`: 配列の要素を生成するテーブル名。`fields` 内の `source` のデフォルトテーブルとなる
- `join`: ルートテーブルとの結合条件を定義する
- `fields`: 配列の各要素のマッピングを再帰的に定義する。ネスト内でも `join` を使用可能

### 4.5 多段結合（join_chain）

中間テーブルを経じて別テーブルに結合する場合、`join_chain` を使用する。

```yaml
response_mapping:
  - field: tags
    type: array
    source_table: tags
    join:
      table: post_tags
      on: posts.id = post_tags.post_id
    join_chain:
      - table: tags
        on: post_tags.tag_id = tags.id
    fields:
      - field: id
        source: tags.id
      - field: name
        source: tags.name
```

- `join_chain`: `join` の次に続く結合を順序付きで定義する
- 各エントリは `table` と `on` で構成される
- 結合の実行順序: `join` → `join_chain[0]` → `join_chain[1]` → …
- 上記の例では `posts → post_tags → tags` という3テーブルの結合を表現する

---

## 5. filters セクション

リクエストパラメータがDBクエリのどの部分になるかを定義する。

### 5.1 WHERE 条件

```yaml
filters:
  - param: status
    maps_to: WHERE
    condition: users.status = :status
```

- `param`: リクエストパラメータ名（OpenAPIのパラメータと照合）
- `maps_to: WHERE` で WHERE 句への対応を示す
- `condition` で実際の条件式を記述（`:status` はパラメータのバインド）

### 5.2 ページネーション

```yaml
filters:
  - param: page
    maps_to: PAGINATION
    strategy: offset
    page_size: 20
    limit_param: limit       # オプション: ページサイズを動的に指定するパラメータ
    max_page_size: 100       # オプション: ページサイズの上限
    cursor_field: created_at # カーソルベース時のキー（strategy: cursor 時のみ）
```

- `maps_to: PAGINATION` でページネーション戦略を示す
- `strategy`: `offset`（LIMIT/OFFSET）/ `cursor`（カーソルベース）
- `page_size`: デフォルトのページサイズ
- `limit_param`: ページサイズを動的に変更するためのパラメータ名。指定されたら OpenAPI のパラメータと照合される
- `max_page_size`: 動的ページサイズの上限値。超過時はバリデーションエラーとなる
- `cursor_field`: `strategy: cursor` の場合、カーソルとなるカラム名を指定する

### 5.3 ソート

```yaml
filters:
  - param: sort
    maps_to: ORDER_BY
    default_column: users.created_at
    default_direction: DESC
    allowed_columns:         # 許容するソート対象カラム一覧
      - users.created_at
      - users.name
      - users.id
    allowed_directions: [ASC, DESC]
```

- `default_column`: ソートカラムが指定されない場合のデフォルト
- `allowed_columns`: 動的カラム指定で許容するカラム一覧。リスト外のカラムを指定した場合はバリデーションエラーとなる
- `allowed_directions`: 許容する並び順

---

## 6. transforms セクション

`response_mapping` で定義されたフィールドの値を変換・加工する。

**優先度規則**: `transforms[].target` と `response_mapping[].field` が同じフィールド名の場合、transforms の結果が最終値となる。つまり `response_mapping` で定義した `source` の値にトランスフォーム変換を適用した結果がレスポンスに返される。

### 6.1 COALESCE（NULL時のフォールバック）

```yaml
transforms:
  - target: display_name
    type: COALESCE
    sources:
      - profiles.display_name
      - users.name
    fallback: "anonymous"
```

- `sources`: 評価順序に従って左から順に NULL でない値を返す
- `fallback`: 全て NULL の場合の固定値

### 6.2 文字列加工

```yaml
transforms:
  - target: full_name
    type: CONCAT
    sources:
      - users.first_name
      - users.last_name
    separator: " "
```

### 6.3 条件分岐

```yaml
transforms:
  - target: status_label
    type: CASE
    source: users.status
    when:
      - value: "active"
        then: "アクティブ"
      - value: "suspended"
        then: "停止中"
    else: "不明"
```

### 6.4 条件付き変換

リクエストパラメータやデータの状態に応じて変換を適用するかどうかを制御する。

```yaml
transforms:
  - target: masked_email
    type: MASK
    source: users.email
    mask_pattern: "***@***.***"
    when:
      # リクエストパラメータによる条件
      - param: viewer_role
        operator: "!="
        value: "admin"
```

- `when`: 変換を適用する条件。複数列記すと AND で評価する
- `when[].param`: リクエストパラメータ名を参照する場合
- `when[].field`: レスポンスの別フィールド値を参照する場合
- `when[].source`: DBカラム値を参照する場合
- `when[].operator`: `==` / `!=` / `>` / `<` / `>=` / `<=` / `in` / `not_in`
- `when` が false の場合、`source` の元の値がそのまま返される

データ状態による条件付き変換の例：

```yaml
transforms:
  - target: body_content
    type: CONDITIONAL_SOURCE
    when:
      - source: posts.status
        operator: "=="
        value: "draft"
    then_source: posts.preview_text
    else_source: posts.body
```

- `then_source` / `else_source`: 条件に応じて異なるカラム値を返す

---

## 7. 完全なサンプル

### 7.1 ユーザー一覧取得

```yaml
version: "0.1"

import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["profiles"]

usecase:
  name: ユーザー一覧取得
  summary: ページネーション付きのユーザー一覧を返す

  response_mapping:
    - field: id
      source: users.id
    - field: name
      source: users.name
    - field: email
      source: users.email
    - field: avatar_url
      source: profiles.avatar_url
      join:
        table: profiles
        on: users.id = profiles.user_id
    - field: display_name
      source: profiles.display_name

  filters:
    - param: status
      maps_to: WHERE
      condition: users.status = :status
    - param: page
      maps_to: PAGINATION
      strategy: offset
      page_size: 20

  transforms:
    - target: display_name
      type: COALESCE
      sources:
        - profiles.display_name
        - users.name
```

### 7.2 投稿詳細取得

```yaml
version: "0.1"

import:
  openapi: ./api.yaml#paths["/posts/{post_id}"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["comments"]
    - ./schema.dbml#tables["likes"]
    - ./schema.dbml#tables["tags"]
    - ./schema.dbml#tables["post_tags"]

usecase:
  name: 投稿詳細取得
  summary: 投稿本文・著者・コメント・いいねCount・タグを返す

  response_mapping:
    - field: id
      source: posts.id
    - field: title
      source: posts.title
    - field: body
      source: posts.body
    - field: author_name
      source: users.name
      join:
        table: users
        on: posts.user_id = users.id
    - field: like_count
      source: likes.id
      join:
        table: likes
        on: posts.id = likes.post_id
      aggregate:
        type: COUNT
        group_by: posts.id
    - field: tags
      type: array
      source_table: tags
      join:
        table: post_tags
        on: posts.id = post_tags.post_id
      join_chain:
        - table: tags
          on: post_tags.tag_id = tags.id
      fields:
        - field: id
          source: tags.id
        - field: name
          source: tags.name
    - field: comments
      type: array
      source_table: comments
      join:
        table: comments
        on: posts.id = comments.post_id
      fields:
        - field: id
          source: comments.id
        - field: body
          source: comments.body
        - field: author_name
          source: comment_author.name
          join:
            table: users
            alias: comment_author
            on: comments.user_id = comment_author.id
        - field: created_at
          source: comments.created_at

  filters:
    - param: post_id
      maps_to: WHERE
      condition: posts.id = :post_id
```

---

## 8. バリデーション規則

パーサーが静的に検証すべき事項：

1. `import.openapi` で参照したレスポンススキーマのフィールドと `response_mapping[].field` が一致すること
2. `import.dbml` で参照したテーブル・カラムが `source` で使われているテーブル・カラムを含むこと
3. `join` で使われるテーブルが `import.dbml` に含まれること（`join_chain` 内も含む）
4. `filters[].param` が `import.openapi` のパラメータに存在すること
5. `transforms[].target` が `response_mapping` のいずれかの `field` に対応していること
6. `join.on` で参照されるテーブル・カラムが存在すること
7. 同じテーブルが異なる結合条件で複数回参照される場合、必ず `alias` が指定されていること
8. `aggregate` を使用するフィールドに `group_by` が明示されているか、ルートテーブルの主キーが推定可能であること
9. `filters[].condition` で使用される `:パラメータ` がすべて `filters[].param` で宣言されていること
10. `transforms[].when[].param` で参照されるパラメータが `import.openapi` に存在すること
11. `source_table` が配列フィールドの `join` で参照されるテーブルと一致していること
12. `allowed_columns` リスト外のカラムが動的ソート指定で使われていないこと

---

## 9. 視覚化

パーサーが生成するビジュアル出力のスコープ：

- **データフロー図**: レスポンスフィールド → 結合 → テーブルのグラフ
- **クエリ概要**: 生成されるSQL的なアウトラインの表示
- **バリデーション結果**: エラー・警告の一覧と該当行のハイライト

---

## 10. 今後の拡張候補（v0.2以降）

- **条件付きフィールド**: 特定条件下でのみレスポンスに含まれるフィールド（`include_when` キー）
- **サブクエリ参照**: スカラーサブクエリや EXISTS チェック
- **ミューテーション定義**: INSERT / UPDATE / DELETE のデータフロー
- **キャッシュヒント**: 結果のキャッシュ戦略の声明
- **ネストされたオブジェクト型マッピング**: 配列でなく単一オブジェクトのネスト
- **Union / Discriminator 型分岐**: レスポンスの型が条件に応じて変わるケース
- **認証コンテキスト**: リクエスト元のユーザー情報に基づくデータフィルタ（例: 自分のデータのみ参照可）
