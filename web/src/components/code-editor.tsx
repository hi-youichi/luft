import CodeMirror from '@uiw/react-codemirror'
import { EditorView } from '@codemirror/view'
import { StreamLanguage } from '@codemirror/language'
import { lua } from '@codemirror/legacy-modes/mode/lua'

const luaLang = StreamLanguage.define(lua)

const darkTheme = EditorView.theme(
  {
    '&': {
      backgroundColor: 'transparent',
      color: '#e4e7eb',
      fontSize: '13px',
      height: '100%',
    },
    '&.cm-editor.cm-focused': {
      outline: 'none',
    },
    '.cm-scroller': {
      fontFamily: "'IBM Plex Mono', 'JetBrains Mono', monospace",
      overflow: 'auto',
    },
    '.cm-content': {
      caretColor: '#00e676',
      padding: '12px 0',
    },
    '.cm-gutters': {
      backgroundColor: 'transparent',
      color: '#5c6578',
      border: 'none',
    },
    '.cm-activeLineGutter': {
      backgroundColor: 'transparent',
      color: '#8b95a7',
    },
    '.cm-activeLine': {
      backgroundColor: 'rgba(255, 255, 255, 0.03)',
    },
    '.cm-cursor': {
      borderLeftColor: '#00e676',
    },
    '.cm-selectionBackground, ::selection': {
      backgroundColor: 'rgba(0, 230, 118, 0.15)',
    },
    '&.cm-focused .cm-selectionBackground': {
      backgroundColor: 'rgba(0, 230, 118, 0.15)',
    },
    '.cm-comment': { color: '#5c6578', fontStyle: 'italic' },
    '.cm-keyword': { color: '#c792ea' },
    '.cm-string': { color: '#c3e88d' },
    '.cm-number': { color: '#f78c6c' },
    '.cm-variable': { color: '#e4e7eb' },
    '.cm-variableName': { color: '#82aaff' },
    '.cm-def': { color: '#82aaff' },
    '.cm-operator': { color: '#89ddff' },
    '.cm-property': { color: '#80cbc4' },
    '.cm-punctuation': { color: '#8b95a7' },
  },
  { dark: true }
)

interface CodeEditorProps {
  value: string
  onChange?: (value: string) => void
  readOnly?: boolean
  className?: string
}

export function CodeEditor({ value, onChange, readOnly, className }: CodeEditorProps) {
  return (
    <CodeMirror
      value={value}
      onChange={onChange}
      extensions={[luaLang, EditorView.lineWrapping]}
      theme={darkTheme}
      readOnly={readOnly}
      className={className}
      height="100%"
      style={{ height: '100%' }}
      basicSetup={{
        lineNumbers: true,
        highlightActiveLine: true,
        highlightActiveLineGutter: true,
        foldGutter: false,
        autocompletion: false,
        searchKeymap: false,
      }}
    />
  )
}
