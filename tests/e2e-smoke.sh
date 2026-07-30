#!/usr/bin/env bash
set -Eeuo pipefail

trap 'status=$?; echo "smoke test failed at line $LINENO: $BASH_COMMAND" >&2; exit "$status"' ERR

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${TAURI_WD_TEST_PORT:-4464}"
STARTUP_TIMEOUT="${TAURI_WD_TEST_STARTUP_TIMEOUT:-90}"
TARGET_DIR="$ROOT/target"
DRIVER="$TARGET_DIR/debug/tauri-wd"
APP="$TARGET_DIR/debug/webdriver-fixture"

cargo build --locked --manifest-path "$ROOT/Cargo.toml" \
  --package tauri-wd
CARGO_TARGET_DIR="$TARGET_DIR" cargo build --locked \
  --manifest-path "$ROOT/tests/fixture/src-tauri/Cargo.toml"

"$DRIVER" --port "$PORT" --startup-timeout "$STARTUP_TIMEOUT" --log info &
DRIVER_PID=$!
SESSION_ID=""
UPLOAD_FILE="$(mktemp)"
SCREENSHOT_FILE="$(mktemp)"
printf 'webdriver-file-upload' > "$UPLOAD_FILE"

cleanup() {
  if [[ -n "$SESSION_ID" ]]; then
    curl -fsS -X DELETE "http://127.0.0.1:$PORT/session/$SESSION_ID" >/dev/null || true
  fi
  kill "$DRIVER_PID" 2>/dev/null || true
  wait "$DRIVER_PID" 2>/dev/null || true
  rm -f "$UPLOAD_FILE" "$SCREENSHOT_FILE"
}
trap cleanup EXIT

for _ in {1..100}; do
  curl -fsS "http://127.0.0.1:$PORT/status" >/dev/null 2>&1 && break
  sleep 0.05
done

SESSION_RESPONSE="$(curl -sS \
  -H 'content-type: application/json' \
  -d "{\"capabilities\":{\"alwaysMatch\":{\"tauri:options\":{\"application\":\"$APP\"}}}}" \
  "http://127.0.0.1:$PORT/session")"
SESSION_ID="$(printf '%s' "$SESSION_RESPONSE" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
if [[ -z "$SESSION_ID" ]]; then
  echo "initial session creation failed: $SESSION_RESPONSE" >&2
  exit 1
fi

curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"implicit":null,"pageLoad":null,"script":null}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null
NULL_TIMEOUTS="$(curl -fsS "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts")"
[[ "$NULL_TIMEOUTS" == *'"implicit":null'* ]]
[[ "$NULL_TIMEOUTS" == *'"pageLoad":null'* ]]
[[ "$NULL_TIMEOUTS" == *'"script":null'* ]]
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"implicit":1000.0,"pageLoad":300000.0,"script":2.0}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null
FLOAT_TIMEOUTS="$(curl -fsS "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts")"
[[ "$FLOAT_TIMEOUTS" == *'"implicit":1000'* ]]
[[ "$FLOAT_TIMEOUTS" == *'"pageLoad":300000'* ]]
[[ "$FLOAT_TIMEOUTS" == *'"script":2'* ]]
INVALID_TIMEOUTS="$(curl -sS -X POST \
  -H 'content-type: application/json' \
  -d '{"implicit":2000,"pageLoad":-1,"script":40}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts")"
[[ "$INVALID_TIMEOUTS" == *'"error":"invalid argument"'* ]]
ATOMIC_TIMEOUTS="$(curl -fsS "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts")"
[[ "$ATOMIC_TIMEOUTS" == *'"implicit":1000'* ]]
[[ "$ATOMIC_TIMEOUTS" == *'"pageLoad":300000'* ]]
[[ "$ATOMIC_TIMEOUTS" == *'"script":2'* ]]

curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"script":20}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"script":null,"implicit":1000}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null
TIMEOUTS="$(curl -fsS "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts")"
[[ "$TIMEOUTS" == *'"script":null'* ]]
ASYNC_AFTER_NULL="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"var done=arguments[arguments.length-1];setTimeout(function(){done(\"after-null\");},75);","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/async")"
[[ "$ASYNC_AFTER_NULL" == *'"value":"after-null"'* ]]
curl -fsS -X POST \
  -H 'content-type: application/json' -d '{"script":20}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null
ASYNC_TIMEOUT="$(curl -sS \
  -H 'content-type: application/json' \
  -d '{"script":"var done=arguments[arguments.length-1];setTimeout(function(){done(\"late\");},75);","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/async")"
[[ "$ASYNC_TIMEOUT" == *'"error":"script timeout"'* ]]
curl -fsS -X POST \
  -H 'content-type: application/json' -d '{"script":null}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"setTimeout(function(){var e=document.createElement(\"span\");e.id=\"implicit-single\";document.body.appendChild(e);},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
IMPLICIT_SINGLE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#implicit-single"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
[[ "$IMPLICIT_SINGLE" == *'"element-6066-11e4-a52e-4f735466cecf"'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"setTimeout(function(){for(var i=0;i<2;i++){var e=document.createElement(\"span\");e.className=\"implicit-many\";document.body.appendChild(e);}},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
IMPLICIT_MANY="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":".implicit-many"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/elements")"
[[ "$(printf '%s' "$IMPLICIT_MANY" | grep -o 'element-6066-11e4-a52e-4f735466cecf' | wc -l | tr -d ' ')" == "2" ]]

PARENT_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#late-parent"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
PARENT_ID="$(printf '%s' "$PARENT_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"document.querySelector(\"#late-parent\").innerHTML=\"\";setTimeout(function(){document.querySelector(\"#late-parent\").innerHTML=\"<i class=from-parent-single></i>\";},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
FROM_PARENT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":".from-parent-single"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$PARENT_ID/element")"
[[ "$FROM_PARENT" == *'"element-6066-11e4-a52e-4f735466cecf"'* ]]
curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"document.querySelector(\"#late-parent\").innerHTML=\"\";setTimeout(function(){document.querySelector(\"#late-parent\").innerHTML=\"<i class=from-parent-many></i><i class=from-parent-many></i>\";},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
FROM_PARENT_MANY="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":".from-parent-many"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$PARENT_ID/elements")"
[[ "$(printf '%s' "$FROM_PARENT_MANY" | grep -o 'element-6066-11e4-a52e-4f735466cecf' | wc -l | tr -d ' ')" == "2" ]]

SHADOW_HOST_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#shadow"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
SHADOW_HOST_ID="$(printf '%s' "$SHADOW_HOST_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
SHADOW_RESPONSE="$(curl -fsS \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$SHADOW_HOST_ID/shadow")"
SHADOW_ID="$(printf '%s' "$SHADOW_RESPONSE" | sed -n 's/.*"shadow-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"var root=document.querySelector(\"#shadow\").shadowRoot;setTimeout(function(){root.innerHTML=\"<b class=from-shadow-single></b>\";},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
FROM_SHADOW="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":".from-shadow-single"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/shadow/$SHADOW_ID/element")"
[[ "$FROM_SHADOW" == *'"element-6066-11e4-a52e-4f735466cecf"'* ]]
curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"var root=document.querySelector(\"#shadow\").shadowRoot;root.innerHTML=\"\";setTimeout(function(){root.innerHTML=\"<b class=from-shadow-many></b><b class=from-shadow-many></b>\";},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
FROM_SHADOW_MANY="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":".from-shadow-many"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/shadow/$SHADOW_ID/elements")"
[[ "$(printf '%s' "$FROM_SHADOW_MANY" | grep -o 'element-6066-11e4-a52e-4f735466cecf' | wc -l | tr -d ' ')" == "2" ]]

ELEMENT_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#increment"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
ELEMENT_ID="$(printf '%s' "$ELEMENT_RESPONSE" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
test -n "$ELEMENT_ID"

for _ in {1..40}; do
  RESULT="$(curl -fsS \
    -H 'content-type: application/json' \
    -d "{\"script\":\"return document.contains(arguments[0].nested[0]);\",\"args\":[{\"nested\":[{\"element-6066-11e4-a52e-4f735466cecf\":\"$ELEMENT_ID\"}]}]}" \
    "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
  [[ "$RESULT" == *'"value":true'* ]]
done

RETURNED_ELEMENT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return {nested:[document.querySelector(\"#increment\")]};","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
RETURNED_ELEMENT_ID="$(printf '%s' "$RETURNED_ELEMENT" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
test -n "$RETURNED_ELEMENT_ID"

curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$RETURNED_ELEMENT_ID/click" >/dev/null
COUNT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#count\").textContent;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$COUNT" == *'"value":"1"'* ]]

CLICK_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#click-sequence"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
CLICK_ID="$(printf '%s' "$CLICK_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST \
  -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$CLICK_ID/click" >/dev/null
CLICK_EVENTS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#click-result\").textContent;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$CLICK_EVENTS" == *'"value":"pointerdown,mousedown,focus,pointerup,mouseup,click"'* ]]

CAPTURE_CLICK_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#capture-click"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$CAPTURE_CLICK_ID/click" >/dev/null
CAPTURE_CLICKS="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return window.captureClicks;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$CAPTURE_CLICKS" == *'"value":1'* ]]

OBSCURER_HIT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"var target=document.querySelector(\"#obscured\");target.scrollIntoView({block:\"center\",inline:\"center\"});var overlay=document.querySelector(\"#obscurer\");document.body.appendChild(overlay);Object.assign(overlay.style,{position:\"fixed\",inset:\"0\",zIndex:\"2147483647\",display:\"block\",pointerEvents:\"auto\",background:\"#000\"});var rect=target.getBoundingClientRect();var hit=document.elementFromPoint(Math.floor((rect.left+rect.right)/2),Math.floor((rect.top+rect.bottom)/2));return hit?hit.id:null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
if [[ "$OBSCURER_HIT" != *'"value":"obscurer"'* ]]; then
  echo "obscured click test was not arranged correctly: $OBSCURER_HIT" >&2
  exit 1
fi

for CASE in \
  'obscured|element click intercepted|center point is obscured' \
  'hidden-button|element not interactable|element has no in-view center point' \
  'no-pointer-button|element not interactable|element does not receive pointer events' \
  'invisible-button|element not interactable|center point is outside the pointer-interactable paint tree'; do
  IFS='|' read -r SELECTOR EXPECTED_ERROR EXPECTED_MESSAGE <<<"$CASE"
  ERROR_ELEMENT="$(curl -fsS \
    -H 'content-type: application/json' \
    -d "{\"using\":\"css selector\",\"value\":\"#$SELECTOR\"}" \
    "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
  ERROR_ELEMENT_ID="$(printf '%s' "$ERROR_ELEMENT" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
  ERROR_RESPONSE="$(curl -sS -X POST \
    -H 'content-type: application/json' -d '{}' \
    "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$ERROR_ELEMENT_ID/click")"
  if [[ "$ERROR_RESPONSE" != *"\"error\":\"$EXPECTED_ERROR\""* ]] ||
    [[ "$ERROR_RESPONSE" != *"$EXPECTED_MESSAGE"* ]]; then
    echo "$SELECTOR returned the wrong WebDriver error: $ERROR_RESPONSE" >&2
    exit 1
  fi
  if [[ "$SELECTOR" == "obscured" ]]; then
    curl -fsS \
      -H 'content-type: application/json' \
      -d '{"script":"document.querySelector(\"#obscurer\").style.display=\"none\";return null;","args":[]}' \
      "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
  fi
done

CHECKBOX_ID="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#checkbox"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$CHECKBOX_ID/click" >/dev/null
CHECKBOX_STATE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#checkbox\").checked;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$CHECKBOX_STATE" == *'"value":true'* ]]

DISABLED_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#disabled-button"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
DISABLED_ID="$(printf '%s' "$DISABLED_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"window.disabledClicks=0;document.querySelector(\"#disabled-button\").addEventListener(\"click\",function(){window.disabledClicks++;});return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$DISABLED_ID/click" >/dev/null
DISABLED_CLICKS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.disabledClicks;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$DISABLED_CLICKS" == *'"value":0'* ]]

KEYS_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#keys"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
KEYS_ID="$(printf '%s' "$KEYS_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"text":"ab\uE012\uE003Z"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$KEYS_ID/value" >/dev/null
KEY_VALUE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#keys\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$KEY_VALUE" == *'"value":"Zb"'* ]]
KEY_EVENTS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#key-result\").textContent;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$KEY_EVENTS" == *'keydown:ArrowLeft'* && "$KEY_EVENTS" == *'keydown:Backspace'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"var el=document.querySelector(\"#keys\");el.value=\"abcd\";el.focus();el.setSelectionRange(4,4);window.keyDetails=[];return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"text":"\uE008\uE012\uE000X"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$KEYS_ID/value" >/dev/null
SHIFT_VALUE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#keys\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$SHIFT_VALUE" == *'"value":"abcX"'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"var el=document.querySelector(\"#keys\");el.value=\"\";el.focus();window.keyDetails=[];return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"text":"\uE050a\uE000\uE007\uE054"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$KEYS_ID/value" >/dev/null
KEY_DETAILS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.keyDetails;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$KEY_DETAILS" == *'keydown:Shift:ShiftRight:2'* ]]
[[ "$KEY_DETAILS" == *'keydown:Enter:NumpadEnter:1'* ]]
[[ "$KEY_DETAILS" == *'keydown:PageUp:Numpad9:3'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"document.querySelector(\"#keys\").value=\"\";document.querySelector(\"#keys-next\").value=\"\";return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"text":"\uE004x"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$KEYS_ID/value" >/dev/null
TAB_VALUE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return [document.activeElement.id,document.querySelector(\"#keys-next\").value];","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$TAB_VALUE" == *'"value":["keys-next","x"]'* ]]

DELAYED_KEYS_ID="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#delayed-keys"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"setTimeout(function(){document.querySelector(\"#delayed-keys\").style.display=\"inline\";},200);return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"ready"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$DELAYED_KEYS_ID/value" >/dev/null
DELAYED_VALUE="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#delayed-keys\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$DELAYED_VALUE" == *'"value":"ready"'* ]]

READONLY_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#readonly"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
READONLY_ID="$(printf '%s' "$READONLY_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST \
  -H 'content-type: application/json' -d '{"text":"x"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$READONLY_ID/value" >/dev/null
READONLY_VALUE="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#readonly\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$READONLY_VALUE" == *'"value":"fixed"'* ]]

BODY_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"body"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -H 'content-type: application/json' \
  -d '{"script":"window.lastDocumentKeyTarget=null;document.addEventListener(\"keydown\",function(event){window.lastDocumentKeyTarget=event.target.tagName;},{once:true,capture:true});return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"q"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$BODY_ID/value" >/dev/null
BODY_KEY_TARGET="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return window.lastDocumentKeyTarget;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$BODY_KEY_TARGET" == *'"value":"BODY"'* ]]

HTML_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"html"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -H 'content-type: application/json' \
  -d '{"script":"window.lastDocumentKeyTarget=null;document.addEventListener(\"keydown\",function(event){window.lastDocumentKeyTarget=event.target.tagName;},{once:true,capture:true});return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"q"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$HTML_ID/value" >/dev/null
HTML_KEY_TARGET="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return window.lastDocumentKeyTarget;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$HTML_KEY_TARGET" == *'"value":"HTML"'* ]]

OPACITY_KEYS_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#opacity-keys"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"q"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$OPACITY_KEYS_ID/value" >/dev/null
OPACITY_VALUE="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#opacity-keys\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$OPACITY_VALUE" == *'"value":"q"'* ]]

OBSCURED_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#obscured"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"q"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$OBSCURED_ID/value" >/dev/null

DATE_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#date"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"01/02/2020"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$DATE_ID/value" >/dev/null
DATE_VALUE="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#date\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$DATE_VALUE" == *'"value":"2020-01-02"'* ]]

FRAME_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#frame"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
curl -fsS -X POST -H 'content-type: application/json' -d '{"text":"frame"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$FRAME_ID/value" >/dev/null
FRAME_INPUT_VALUE="$(curl -fsS -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#frame\").contentDocument.querySelector(\"#frame-input\").value;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$FRAME_INPUT_VALUE" == *'"value":"frame"'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"window.actionEvents=[];document.querySelector(\"#keys\").focus();return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"actions":[{"type":"key","id":"a","actions":[{"type":"keyDown","value":"a"},{"type":"keyUp","value":"a"}]},{"type":"key","id":"b","actions":[{"type":"keyDown","value":"b"},{"type":"keyUp","value":"b"}]}]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
ACTION_EVENTS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.actionEvents;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$ACTION_EVENTS" == *'"value":["keydown:a","keydown:b","keyup:a","keyup:b"]'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"window.keyDetails=[];return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"actions":[{"type":"key","id":"special","actions":[{"type":"keyDown","value":"\uE050"},{"type":"keyDown","value":"\uE054"},{"type":"keyUp","value":"\uE054"},{"type":"keyUp","value":"\uE050"},{"type":"keyDown","value":"\uE006"},{"type":"keyUp","value":"\uE006"},{"type":"keyDown","value":"\uE007"},{"type":"keyUp","value":"\uE007"}]}]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
ACTION_KEY_DETAILS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.keyDetails;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$ACTION_KEY_DETAILS" == *'keydown:Shift:ShiftRight:2'* ]]
[[ "$ACTION_KEY_DETAILS" == *'keydown:PageUp:Numpad9:3'* ]]
[[ "$ACTION_KEY_DETAILS" == *'keydown:Enter:Enter:0'* ]]
[[ "$ACTION_KEY_DETAILS" == *'keydown:Enter:NumpadEnter:1'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"window.actionEvents=[];return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"actions":[{"type":"key","id":"keyup-only","actions":[{"type":"keyUp","value":"z"}]}]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
KEYUP_ONLY_EVENTS="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.actionEvents;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$KEYUP_ONLY_EVENTS" == *'"value":[]'* ]]

SCREENSHOT_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#screenshot-target"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
SCREENSHOT_ID="$(printf '%s' "$SCREENSHOT_RESPONSE" | sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
SCREENSHOT="$(curl -fsS \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$SCREENSHOT_ID/screenshot")"
SCREENSHOT_BASE64="$(printf '%s' "$SCREENSHOT" | sed -n 's/^{"value":"\([^"]*\)"}$/\1/p')"
printf '%s' "$SCREENSHOT_BASE64" | openssl base64 -d -A > "$SCREENSHOT_FILE"
read -r W1 W2 W3 W4 H1 H2 H3 H4 < <(od -An -tu1 -j16 -N8 "$SCREENSHOT_FILE")
SCREENSHOT_WIDTH=$((W1 * 16777216 + W2 * 65536 + W3 * 256 + W4))
SCREENSHOT_HEIGHT=$((H1 * 16777216 + H2 * 65536 + H3 * 256 + H4))
(( SCREENSHOT_WIDTH >= 32 && SCREENSHOT_WIDTH <= 256 ))
(( SCREENSHOT_HEIGHT >= 16 && SCREENSHOT_HEIGHT <= 128 ))
(( SCREENSHOT_WIDTH > SCREENSHOT_HEIGHT ))

curl -fsS -X POST -H 'content-type: application/json' \
  -d "{\"id\":{\"element-6066-11e4-a52e-4f735466cecf\":\"$FRAME_ID\"}}" \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/frame" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"script":5000}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null
FRAME_ASYNC="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"arguments[arguments.length-1](\"frame-async\");","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/async")"
[[ "$FRAME_ASYNC" == *'"value":"frame-async"'* ]]
FRAME_SHOT_ID="$(curl -fsS -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#frame-shot"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
FRAME_SCREENSHOT="$(curl -fsS \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$FRAME_SHOT_ID/screenshot")"
FRAME_SCREENSHOT_BASE64="$(printf '%s' "$FRAME_SCREENSHOT" | sed -n 's/^{"value":"\([^"]*\)"}$/\1/p')"
printf '%s' "$FRAME_SCREENSHOT_BASE64" | openssl base64 -d -A > "$SCREENSHOT_FILE"
read -r W1 W2 W3 W4 H1 H2 H3 H4 < <(od -An -tu1 -j16 -N8 "$SCREENSHOT_FILE")
FRAME_SHOT_WIDTH=$((W1 * 16777216 + W2 * 65536 + W3 * 256 + W4))
FRAME_SHOT_HEIGHT=$((H1 * 16777216 + H2 * 65536 + H3 * 256 + H4))
(( FRAME_SHOT_WIDTH >= 20 && FRAME_SHOT_WIDTH <= 160 ))
(( FRAME_SHOT_HEIGHT >= 10 && FRAME_SHOT_HEIGHT <= 80 ))
FRAME_SHOT_COLOR="$(curl -fsS \
  -H 'content-type: application/json' \
  -d "{\"script\":\"var done=arguments[arguments.length-1];var image=new Image();image.onload=function(){var canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;var context=canvas.getContext('2d');context.drawImage(image,0,0);done(Array.from(context.getImageData(Math.floor(image.width/2),Math.floor(image.height/2),1,1).data));};image.onerror=function(){done('error');};image.src='data:image/png;base64,'+arguments[0];\",\"args\":[\"$FRAME_SCREENSHOT_BASE64\"]}" \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/async")"
[[ "$FRAME_SHOT_COLOR" == *'"value":[204,0,0,255]'* ]]
curl -fsS -X POST -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/frame/parent" >/dev/null
curl -fsS -X POST -H 'content-type: application/json' \
  -d '{"script":null}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/timeouts" >/dev/null

UPLOAD_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#upload"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
UPLOAD_ELEMENT_ID="$(printf '%s' "$UPLOAD_RESPONSE" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
test -n "$UPLOAD_ELEMENT_ID"
UPLOAD_CLICK_ERROR="$(curl -sS -X POST \
  -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$UPLOAD_ELEMENT_ID/click")"
[[ "$UPLOAD_CLICK_ERROR" == *'"error":"invalid argument"'* ]]
UPLOAD_JSON="$(printf '%s' "$UPLOAD_FILE" | sed 's/\\/\\\\/g; s/"/\\"/g')"
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d "{\"text\":\"$UPLOAD_JSON\",\"value\":[]}" \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element/$UPLOAD_ELEMENT_ID/value" >/dev/null
for _ in {1..40}; do
  UPLOAD_RESULT="$(curl -fsS \
    -H 'content-type: application/json' \
    -d '{"script":"return document.querySelector(\"#upload-result\").textContent;","args":[]}' \
    "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
  [[ "$UPLOAD_RESULT" == *'webdriver-file-upload'* ]] && break
  sleep 0.05
done
[[ "$UPLOAD_RESULT" == *'webdriver-file-upload'* ]]

POINTER_RESPONSE="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"using":"css selector","value":"#pointer-pad"}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/element")"
POINTER_ELEMENT_ID="$(printf '%s' "$POINTER_RESPONSE" | \
  sed -n 's/.*"element-6066-11e4-a52e-4f735466cecf":"\([^"]*\)".*/\1/p')"
test -n "$POINTER_ELEMENT_ID"
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d "{\"actions\":[{\"type\":\"pointer\",\"id\":\"mouse\",\"actions\":[{\"type\":\"pointerMove\",\"x\":0,\"y\":0,\"origin\":{\"element-6066-11e4-a52e-4f735466cecf\":\"$POINTER_ELEMENT_ID\"}},{\"type\":\"pointerDown\",\"button\":0},{\"type\":\"pause\",\"duration\":50},{\"type\":\"pointerMove\",\"x\":25,\"y\":10,\"origin\":\"pointer\"},{\"type\":\"pointerUp\",\"button\":0}]}]}" \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
POINTER_RESULT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return document.querySelector(\"#pointer-result\").textContent;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$POINTER_RESULT" == *'"value":"move:25,10:1:up:0"'* ]]

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"window.pointerMoveCount=0;return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d "{\"actions\":[{\"type\":\"pointer\",\"id\":\"interpolated-mouse\",\"actions\":[{\"type\":\"pointerMove\",\"x\":0,\"y\":0,\"origin\":{\"element-6066-11e4-a52e-4f735466cecf\":\"$POINTER_ELEMENT_ID\"}},{\"type\":\"pointerMove\",\"x\":40,\"y\":0,\"duration\":80,\"origin\":\"pointer\"}]}]}" \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
POINTER_MOVE_COUNT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.pointerMoveCount;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
POINTER_MOVE_COUNT_VALUE="$(printf '%s' "$POINTER_MOVE_COUNT" | sed -n 's/.*"value":\([0-9][0-9]*\).*/\1/p')"
(( POINTER_MOVE_COUNT_VALUE > 2 ))

curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"window.pointerClickCount=0;return null;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d "{\"actions\":[{\"type\":\"pointer\",\"id\":\"persistent-mouse\",\"actions\":[{\"type\":\"pointerMove\",\"x\":0,\"y\":0,\"origin\":{\"element-6066-11e4-a52e-4f735466cecf\":\"$POINTER_ELEMENT_ID\"}},{\"type\":\"pointerDown\",\"button\":0}]}]}" \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
curl -fsS -X POST \
  -H 'content-type: application/json' \
  -d '{"actions":[{"type":"pointer","id":"persistent-mouse","actions":[{"type":"pointerUp","button":0}]}]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/actions" >/dev/null
POINTER_CLICK_COUNT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"return window.pointerClickCount;","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/sync")"
[[ "$POINTER_CLICK_COUNT" == *'"value":1'* ]]

ASYNC_ELEMENT="$(curl -fsS \
  -H 'content-type: application/json' \
  -d '{"script":"arguments[arguments.length - 1](document.querySelector(\"#name\"));","args":[]}' \
  "http://127.0.0.1:$PORT/session/$SESSION_ID/execute/async")"
[[ "$ASYNC_ELEMENT" == *'"element-6066-11e4-a52e-4f735466cecf"'* ]]

for _ in {1..5}; do
  curl -fsS -X POST \
    -H 'content-type: application/json' \
    -d '{}' \
    "http://127.0.0.1:$PORT/session/$SESSION_ID/refresh" >/dev/null
  TITLE="$(curl -fsS "http://127.0.0.1:$PORT/session/$SESSION_ID/title")"
  [[ "$TITLE" == *'"value":"WebDriver Fixture"'* ]]
done

curl -fsS -X DELETE "http://127.0.0.1:$PORT/session/$SESSION_ID" >/dev/null
SESSION_ID=""

for _ in {1..4}; do
  SESSION_RESPONSE="$(curl -sS \
    -H 'content-type: application/json' \
    -d "{\"capabilities\":{\"alwaysMatch\":{\"tauri:options\":{\"application\":\"$APP\"}}}}" \
    "http://127.0.0.1:$PORT/session")"
  SESSION_ID="$(printf '%s' "$SESSION_RESPONSE" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')"
  if [[ -z "$SESSION_ID" ]]; then
    echo "repeat session creation failed: $SESSION_RESPONSE" >&2
    exit 1
  fi
  TITLE="$(curl -fsS "http://127.0.0.1:$PORT/session/$SESSION_ID/title")"
  [[ "$TITLE" == *'"value":"WebDriver Fixture"'* ]]
  curl -fsS -X DELETE "http://127.0.0.1:$PORT/session/$SESSION_ID" >/dev/null
  SESSION_ID=""
done

echo "native WebDriver smoke test passed"
