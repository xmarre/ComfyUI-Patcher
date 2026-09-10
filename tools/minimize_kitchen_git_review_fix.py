from pathlib import Path

p = Path("src-tauri/src/git.rs")
text = p.read_text(encoding="utf-8")

old = '''    trimmed == "__pycache__"\n        || trimmed.starts_with("__pycache__/")\n'''
new = '''    trimmed == "__pycache__"\n        || trimmed == ".patcher-build"\n        || trimmed.starts_with(".patcher-build/")\n        || trimmed.starts_with("__pycache__/")\n'''
if text.count(old) != 1:
    raise SystemExit(f"ignore insertion: expected 1 match, found {text.count(old)}")
text = text.replace(old, new, 1)

anchor = '''    #[test]\n    fn accepts_single_directory_name() {\n'''
test = '''    #[test]\n    fn ignores_patcher_build_artifacts_but_not_similar_user_paths() {\n        assert!(is_ignorable_generated_untracked_path(".patcher-build"));\n        assert!(is_ignorable_generated_untracked_path(\n            ".patcher-build/operation-123/comfy_kitchen.whl"\n        ));\n        assert!(!is_ignorable_generated_untracked_path(\n            ".patcher-builder/user-file.txt"\n        ));\n        assert!(!is_ignorable_generated_untracked_path(\n            "nested/.patcher-build/user-file.txt"\n        ));\n    }\n\n    #[test]\n    fn accepts_single_directory_name() {\n'''
if text.count(anchor) != 1:
    raise SystemExit(f"test insertion: expected 1 match, found {text.count(anchor)}")
text = text.replace(anchor, test, 1)
p.write_text(text, encoding="utf-8", newline="\n")
print("Minimal git.rs Kitchen review fix applied")
