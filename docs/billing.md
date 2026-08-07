# Polar billing

Slimeの配布ビルドは、PolarのsubscriptionとLicense Keys benefitを使って利用権を確認する。
入力内容、ユーザー辞書、入力履歴は課金処理に送らない。Polarへ送るのは利用者が入力したライセンスキーと、公開情報であるorganization ID / benefit IDだけである。

## Polar production setup

Polar productionには次のリソースを作成済み。

1. Organization `Slime` (`slime-ime`): `6a332eb2-129e-4a1b-92fe-8ec7777780df`
2. `Slime Monthly`: 固定価格 `JPY 200`、請求間隔 `1 month`、trial `14 days`
3. License Keys benefit `Slime`: 公開resource UUID `e014726e-23a8-4a99-a212-e44adc349c1e`
4. Checkout Link: `https://buy.polar.sh/polar_cl_asAsYJgLTkAhiius7JmEbgTIPRpEb4r4H8Unz2x9us2`

License Keys benefitにactivation limitは設定しない。Checkout LinkのReturn URLとSuccess URLは `https://slime.unvalley.me/` とし、短命なCheckout Session URLではなく上記の永続LinkをアプリとLandingで使う。

Polarのtrial中と契約中はbenefitが有効なので、発行されたlicense keyのvalidationが成功する。trial終了後の支払い失敗、解約、失効はPolarがbenefitへ反映する。Slimeは起動時に再確認し、最後の成功から7日間だけ一時的なオフライン利用を許可する。

## Production build

productionのorganization ID、benefit ID、Checkout Linkは公開設定としてbuild scriptに固定されている。秘密のAPI tokenはアプリへ埋め込まない。

```sh
SLIME_BILLING_ENVIRONMENT=production just build-macos
```

production値は同名の環境変数で明示的に上書きできる。`sandbox`で必須値がない場合、bundle作成は失敗する。通常のローカルbuildは `development`で課金確認を迂回する。この迂回は生成済みtrialやKeychainの状態を書き換えない。

## Sandbox

本番とは別のSandbox product、benefit、Checkout Linkを作り、次のように起動する。

```sh
SLIME_BILLING_ENVIRONMENT=sandbox \
SLIME_POLAR_ORGANIZATION_ID=<sandbox-organization-uuid> \
SLIME_POLAR_BENEFIT_ID=<sandbox-benefit-uuid> \
SLIME_POLAR_CHECKOUT_URL=<sandbox-checkout-link> \
just build-macos
```

Sandboxでは `https://sandbox-api.polar.sh` のvalidation endpointと、別のKeychain / UserDefaults名前空間を使う。

## Landing

```sh
cd landing
pnpm install --frozen-lockfile
pnpm build
pnpm check
pnpm run deploy
```

production Checkoutでは `14 days free` と `¥200/month` が表示される。新OrganizationはAccount Review完了までtest modeであり、有料注文は受け付けない。公開サイト、本人確認、振込先口座、サポートメールを揃えてPolarへreviewを提出する。

公開前に、実際のSandbox購入、14日trial表示、キー発行、アプリでのactivation、解約後の失効を一続きで確認する。mock testだけを実購入の証明にはしない。productionでは実カードを使ったtest購入を行わない。
