package cuelight;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal JSON reader/writer, so the template needs no build tooling: plain {@code javac} works.
 *
 * Values map to: {@code Map<String,Object>}, {@code List<Object>}, {@code String}, {@code Double},
 * {@code Boolean}, {@code null}. Insertion order is preserved on write, which keeps runs
 * byte-reproducible.
 */
public final class Json {
    private final String s;
    private int i;

    private Json(String s) { this.s = s; }

    public static Object parse(String text) {
        Json p = new Json(text);
        p.ws();
        return p.value();
    }

    private void ws() { while (i < s.length() && Character.isWhitespace(s.charAt(i))) i++; }

    private Object value() {
        char c = s.charAt(i);
        switch (c) {
            case '{': return object();
            case '[': return array();
            case '"': return string();
            case 't': i += 4; return Boolean.TRUE;
            case 'f': i += 5; return Boolean.FALSE;
            case 'n': i += 4; return null;
            default:  return number();
        }
    }

    private Map<String, Object> object() {
        Map<String, Object> m = new LinkedHashMap<>();
        i++; ws();
        if (s.charAt(i) == '}') { i++; return m; }
        while (true) {
            ws();
            String k = string();
            ws(); i++;              // ':'
            ws();
            m.put(k, value());
            ws();
            if (s.charAt(i) == ',') { i++; continue; }
            i++; return m;          // '}'
        }
    }

    private List<Object> array() {
        List<Object> l = new ArrayList<>();
        i++; ws();
        if (s.charAt(i) == ']') { i++; return l; }
        while (true) {
            ws();
            l.add(value());
            ws();
            if (s.charAt(i) == ',') { i++; continue; }
            i++; return l;          // ']'
        }
    }

    private String string() {
        StringBuilder b = new StringBuilder();
        i++;                        // opening quote
        while (true) {
            char c = s.charAt(i++);
            if (c == '"') return b.toString();
            if (c != '\\') { b.append(c); continue; }
            char e = s.charAt(i++);
            switch (e) {
                case 'n': b.append('\n'); break;
                case 't': b.append('\t'); break;
                case 'r': b.append('\r'); break;
                case 'b': b.append('\b'); break;
                case 'f': b.append('\f'); break;
                case 'u': b.append((char) Integer.parseInt(s.substring(i, i + 4), 16)); i += 4; break;
                default:  b.append(e);
            }
        }
    }

    private Double number() {
        int start = i;
        while (i < s.length() && "-+.eE0123456789".indexOf(s.charAt(i)) >= 0) i++;
        return Double.valueOf(s.substring(start, i));
    }

    public static String write(Object v) {
        StringBuilder b = new StringBuilder();
        writeTo(v, b);
        return b.toString();
    }

    @SuppressWarnings("unchecked")
    private static void writeTo(Object v, StringBuilder b) {
        if (v == null) { b.append("null"); return; }
        if (v instanceof String) { quote((String) v, b); return; }
        if (v instanceof Boolean) { b.append(v); return; }
        if (v instanceof Number) {
            double d = ((Number) v).doubleValue();
            if (d == Math.rint(d) && !Double.isInfinite(d)) b.append((long) d);
            else b.append(d);
            return;
        }
        if (v instanceof Map) {
            b.append('{');
            boolean first = true;
            for (Map.Entry<String, Object> e : ((Map<String, Object>) v).entrySet()) {
                if (!first) b.append(',');
                first = false;
                quote(e.getKey(), b);
                b.append(':');
                writeTo(e.getValue(), b);
            }
            b.append('}');
            return;
        }
        if (v instanceof List) {
            b.append('[');
            boolean first = true;
            for (Object o : (List<Object>) v) {
                if (!first) b.append(',');
                first = false;
                writeTo(o, b);
            }
            b.append(']');
            return;
        }
        throw new IllegalArgumentException("cannot serialise " + v.getClass());
    }

    private static void quote(String s, StringBuilder b) {
        b.append('"');
        for (int k = 0; k < s.length(); k++) {
            char c = s.charAt(k);
            switch (c) {
                case '"':  b.append("\\\""); break;
                case '\\': b.append("\\\\"); break;
                case '\n': b.append("\\n"); break;
                case '\r': b.append("\\r"); break;
                case '\t': b.append("\\t"); break;
                default:
                    if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
                    else b.append(c);
            }
        }
        b.append('"');
    }
}
