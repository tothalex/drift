; drift's injections: embedded <style> and <script> bodies highlight
; with the css / javascript grammars — when those languages are
; installed; without them the blocks simply stay plain.

((style_element
  (raw_text) @injection.content)
 (#set! injection.language "css"))

((script_element
  (raw_text) @injection.content)
 (#set! injection.language "javascript"))
