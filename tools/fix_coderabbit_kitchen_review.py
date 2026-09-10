from pathlib import Path


def replace_once(path, old, new, label):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, found {count}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


replace_once(
    "src-tauri/src/git.rs",
    '''    trimmed == "__pycache__"\n        || trimmed.starts_with("__pycache__/")\n        || trimmed.contains("/__pycache__/")\n        || trimmed.ends_with("/__pycache__")\n        || trimmed.ends_with(".pyc")\n        || trimmed.ends_with(".pyo")\n''',
    '''    trimmed == ".patcher-build"\n        || trimmed.starts_with(".patcher-build/")\n        || trimmed == "__pycache__"\n        || trimmed.starts_with("__pycache__/")\n        || trimmed.contains("/__pycache__/")\n        || trimmed.ends_with("/__pycache__")\n        || trimmed.ends_with(".pyc")\n        || trimmed.ends_with(".pyo")\n''',
    "ignore root Patcher Kitchen build artifacts",
)

replace_once(
    "src-tauri/src/git.rs",
    '''    #[test]\n    fn accepts_single_directory_name() {\n''',
    '''    #[test]\n    fn ignores_patcher_build_artifacts_but_not_similar_user_paths() {\n        assert!(is_ignorable_generated_untracked_path(".patcher-build"));\n        assert!(is_ignorable_generated_untracked_path(\n            ".patcher-build/operation-123/comfy_kitchen.whl"\n        ));\n        assert!(!is_ignorable_generated_untracked_path(\n            ".patcher-builder/user-file.txt"\n        ));\n        assert!(!is_ignorable_generated_untracked_path(\n            "nested/.patcher-build/user-file.txt"\n        ));\n    }\n\n    #[test]\n    fn accepts_single_directory_name() {\n''',
    "Patcher build dirtiness regression test",
)

replace_once(
    "src/App.tsx",
    '''                setCorePreview(null);\n                setFrontendPreview(null);\n                setKitchenActionPreview(null);\n                setNodePreview(null);\n''',
    '''                setCorePreview(null);\n                setFrontendPreview(null);\n                kitchenPreviewRequestSeq.current += 1;\n                setKitchenActionPreview(null);\n                setNodePreview(null);\n''',
    "invalidate Kitchen preview after registration update",
)

replace_once(
    "src/App.tsx",
    '''                      });\n                      setKitchenActionPreview(null);\n                      setKitchenPreviewError(null);\n''',
    '''                      });\n                      kitchenPreviewRequestSeq.current += 1;\n                      setKitchenActionPreview(null);\n                      setKitchenPreviewError(null);\n''',
    "invalidate Kitchen preview after source installation",
)

print("Final Kitchen review fixes applied")
