import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml
import prepare_linux_clap
import prepare_macos_clap
import verify_clap_worker
from release_state import published
from validate_release_assets import validate

ROOT = Path(__file__).resolve().parent.parent


class ReleaseSafetyTest(unittest.TestCase):
    def test_worker_smoke_check_bounds_time_and_rejects_invalid_output(self):
        for output in ('', '{"ready":true}\n', '{"ready":true}\n{"vectors":[[1]]}\n'):
            result = subprocess.CompletedProcess([], 0, output, '')
            with patch.object(verify_clap_worker.subprocess, 'run', return_value=result) as run:
                with self.assertRaises(ValueError): verify_clap_worker.verify('worker', 'model')
                self.assertEqual(run.call_args.kwargs['timeout'], 180)
        with patch.object(verify_clap_worker.subprocess, 'run', side_effect=subprocess.TimeoutExpired('worker', 180)):
            with self.assertRaises(subprocess.TimeoutExpired): verify_clap_worker.verify('worker', 'model')

    def test_release_lookup_distinguishes_published_drafts_missing_and_api_errors(self):
        for draft in (True, False):
            result = subprocess.CompletedProcess([], 0, json.dumps({'isDraft': draft}), '')
            self.assertEqual(published(result), not draft)
        self.assertFalse(published(subprocess.CompletedProcess([], 1, '', 'release not found')))
        for message in ('HTTP 403: Forbidden', 'network unavailable', 'timeout'):
            with self.assertRaises(RuntimeError): published(subprocess.CompletedProcess([], 1, '', message))
        for body in ('{}', '{"isDraft":"false"}', 'invalid'):
            with self.assertRaises(ValueError): published(subprocess.CompletedProcess([], 0, body, ''))

    def test_publication_requires_every_installer_and_signature(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); version = '0.4.13'
            with self.assertRaises(ValueError): validate(root, version)
            signed = [f'Fastcloud_{version}_x64-{kind}.exe' for kind in ('setup', 'update')]
            for arch in ('aarch64', 'x86_64'):
                (root / f'Fastcloud_{version}_macos-{arch}.dmg').write_bytes(b'fixture')
                signed.extend(f'Fastcloud_{version}_macos-{arch}-{kind}.app.tar.gz' for kind in ('setup', 'update'))
            for name in signed:
                (root / name).write_bytes(b'fixture'); (root / (name + '.sig')).write_text('signature')
            # Linux packages are mandatory assets: no release without them.
            with self.assertRaises(ValueError): validate(root, version)
            for suffix in ('.deb', '.AppImage'):
                (root / f'Fastcloud_{version}_linux-x86_64{suffix}').write_bytes(b'fixture')
            validate(root, version)
            # Release runs stage Linux packages in x86_64/, not in the release root.
            (root / 'x86_64').mkdir()
            for suffix in ('.deb', '.AppImage'):
                (root / f'Fastcloud_{version}_linux-x86_64{suffix}').rename(root / 'x86_64' / f'Fastcloud_{version}_linux-x86_64{suffix}')
            validate(root, version)
            (root / (signed[-1] + '.sig')).write_text(' ')
            with self.assertRaises(ValueError): validate(root, version)

    def test_parallel_builds_still_gate_publication_and_warm_matching_release_caches(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text(encoding='utf-8'))
        jobs = workflow['jobs']
        self.assertNotIn('needs', jobs['release-windows'])
        self.assertNotIn('needs', jobs['macos'])
        self.assertEqual(set(jobs['publish']['needs']), {'desktop', 'macos', 'release-windows', 'linux-release'})
        self.assertEqual(jobs['publish']['permissions']['contents'], 'write')
        self.assertNotIn('permissions', jobs['release-windows'])
        steps = jobs['publish']['steps']
        lookup = next(i for i, step in enumerate(steps) if step.get('run') == 'python tools/release_state.py')
        validate_index = next(i for i, step in enumerate(steps) if step.get('run') == 'python tools/validate_release_assets.py')
        upload = next(i for i, step in enumerate(steps) if step.get('uses', '').startswith('softprops/'))
        publish_index = next(i for i, step in enumerate(steps) if '--draft=false' in step.get('run', ''))
        notify = next(i for i, step in enumerate(steps) if step.get('run') == 'python tools/notify_release.py')
        self.assertTrue(lookup < validate_index < upload < publish_index < notify)
        self.assertTrue(steps[upload]['with']['draft'])
        for step in steps[lookup+1:notify]: self.assertIn("steps.release.outputs.published != 'true'", step.get('if', ''))
        warm = yaml.safe_load((ROOT / '.github/workflows/release-cache.yml').read_text(encoding='utf-8'))['jobs']['warm']
        self.assertNotIn('secrets.', json.dumps(warm))
        self.assertIn('head_repository.full_name == github.repository', warm['if'])
        self.assertIn("workflow_run.conclusion == 'success'", warm['if'])
        caches = {row['cache'] for row in warm['strategy']['matrix']['include']}
        self.assertEqual(caches, {'release-windows-x64', 'release-macos-aarch64', 'release-macos-x86_64'})
        for job in ('desktop', 'release-windows', 'macos'):
            toolchains = [step['with']['toolchain'] for step in jobs[job]['steps'] if step.get('uses', '').startswith('dtolnay/')]
            self.assertEqual(toolchains, ['1.97.1'])
        self.assertTrue(any(step.get('uses', '').startswith('dtolnay/') and step['with']['toolchain'] == '1.97.1' for step in warm['steps']))

    def test_model_and_worker_hits_skip_dependency_installs_but_still_verify_native_worker(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); bundle = root / 'desktop/src-tauri/resources/clap'
            hashes = {}
            for name in ('config.json','preprocessor_config.json','tokenizer.json','tokenizer_config.json','special_tokens_map.json','vocab.json','merges.txt','onnx/audio_model_quantized.onnx','onnx/text_model_quantized.onnx'):
                path = bundle / 'model' / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'fixture')
                if name.startswith('onnx/'): hashes[path.name] = hashlib.sha256(b'fixture').hexdigest()
            worker = bundle / 'worker-lite/fastcloud-clap/fastcloud-clap'; worker.parent.mkdir(parents=True); worker.write_bytes(b'worker')
            licenses = root / 'assets/licenses'; licenses.mkdir(parents=True)
            for name in ('NOTICE-CLAP.txt', 'LICENSE-APACHE-2.0.txt'): (licenses / name).write_text('fixture')
            response = subprocess.CompletedProcess([], 0, json.dumps({'ready': True})+'\n'+json.dumps({'vectors': [[0]*512]})+'\n', '')
            with patch.object(prepare_macos_clap, 'ROOT', root), patch.object(prepare_macos_clap, 'BUNDLE', bundle), patch.object(prepare_macos_clap, 'HASHES', hashes), patch.object(prepare_macos_clap.sys, 'platform', 'darwin'), patch.object(prepare_macos_clap.subprocess, 'run', return_value=response) as run:
                prepare_macos_clap.main()
            run.assert_called_once()
            self.assertEqual(run.call_args.args[0], [str(worker), str(bundle / 'model')])

    def test_linux_preparation_patches_numpy_runtime_and_verifies_cached_worker(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); bundle = root / 'desktop/src-tauri/resources/clap'
            hashes = {}
            for name in ('config.json','preprocessor_config.json','tokenizer.json','tokenizer_config.json','special_tokens_map.json','vocab.json','merges.txt','onnx/audio_model_quantized.onnx','onnx/text_model_quantized.onnx'):
                path = bundle / 'model' / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'fixture')
                if name.startswith('onnx/'): hashes[path.name] = hashlib.sha256(b'fixture').hexdigest()
            worker = bundle / 'worker-lite/fastcloud-clap/fastcloud-clap'; worker.parent.mkdir(parents=True); worker.write_bytes(b'worker')
            numpy_libs = bundle / 'worker-lite/fastcloud-clap/_internal/numpy.libs'
            numpy_libs.mkdir(parents=True)
            (numpy_libs / 'libgfortran-test.so.5').write_bytes(b'gfortran')
            licenses = root / 'assets/licenses'; licenses.mkdir(parents=True)
            for name in ('NOTICE-CLAP.txt', 'LICENSE-APACHE-2.0.txt'): (licenses / name).write_text('fixture')
            response = subprocess.CompletedProcess([], 0, json.dumps({'ready': True})+'\n'+json.dumps({'vectors': [[0]*512]})+'\n', '')
            patched = []
            with patch.object(prepare_linux_clap, 'ROOT', root), patch.object(prepare_linux_clap, 'BUNDLE', bundle), patch.object(prepare_linux_clap, 'HASHES', hashes), patch.object(prepare_linux_clap.sys, 'platform', 'linux'), patch.object(prepare_linux_clap, 'patch_rpath', side_effect=lambda paths: patched.extend(paths) or paths), patch.object(prepare_linux_clap.subprocess, 'run', return_value=response) as run:
                prepare_linux_clap.main()
            run.assert_called_once()
            self.assertEqual(run.call_args.args[0], [str(worker), str(bundle / 'model')])
            config = json.loads((root / 'desktop/src-tauri/tauri.private.conf.json').read_text(encoding='utf-8'))
            self.assertIn('resources/clap/worker-lite/fastcloud-clap/**/*', config['bundle']['resources'])
            # linuxdeploy inspects each vendored library on its own, so numpy's
            # mangled runtime has to carry its own $ORIGIN search path.
            self.assertEqual([path.name for path in patched], ['libgfortran-test.so.5'])

    def test_linux_preparation_rejects_a_worker_without_numpy_runtime(self):
        import prepare_linux_clap
        with tempfile.TemporaryDirectory() as directory:
            # A worker without numpy.libs cannot be bundled: linuxdeploy would
            # only fail much later with an opaque linuxdeploy error.
            with self.assertRaises(SystemExit):
                prepare_linux_clap.patch_numpy_runtime(Path(directory))

    def test_linux_workflow_bundles_the_verified_native_clap_worker(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/linux.yml').read_text(encoding='utf-8'))
        jobs = workflow['jobs']
        self.assertEqual(list(jobs), ['linux'])
        steps = jobs['linux']['steps']
        prepare = next(i for i, step in enumerate(steps) if step.get('uses') == './.github/actions/clap-resources')
        builds = [i for i, step in enumerate(steps) if 'tauri.linux.conf.json' in step.get('run', '')]
        self.assertEqual(len(builds), 2)
        self.assertLess(prepare, min(builds))
        for index in builds:
            self.assertIn('tauri.private.conf.json', steps[index]['run'])
        # linuxdeploy is an AppImage: extracting it avoids FUSE, and verbose
        # output keeps its diagnostics when bundling fails.
        self.assertEqual(workflow['env']['APPIMAGE_EXTRACT_AND_RUN'], '1')
        appimage = next(step['run'] for step in steps if '--bundles appimage' in step.get('run', ''))
        self.assertIn('--verbose', appimage)
        # Release installers are renamed before they are uploaded.
        staging = next(step for step in steps if 'stage_linux_release.py' in step.get('run', ''))
        self.assertIn('$BUNDLE', staging['run'])
        # The release workflow calls this Linux workflow for tagged builds, so
        # the published packages come from the very same tested steps.
        triggers = workflow.get('on') or workflow.get(True)
        self.assertIn('workflow_call', triggers)
        self.assertIn('artifact', triggers['workflow_call']['inputs'])
        ci = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text(encoding='utf-8'))
        caller = ci['jobs']['linux-release']
        self.assertEqual(caller['uses'], './.github/workflows/linux.yml')
        self.assertEqual(caller['if'], "startsWith(github.ref, 'refs/tags/v')")
        self.assertEqual(caller['with'], {'artifact': 'linux-release'})
        self.assertIn('linux-release', ci['jobs']['publish']['needs'])
        uploads = [step for step in steps if step.get('uses', '').startswith('actions/upload-artifact')]
        self.assertEqual({upload['with']['name'] for upload in uploads}, {
            "${{ inputs.artifact || 'veirune-linux-x86_64' }}",
            "${{ inputs.artifact || 'veirune-linux-x86_64' }}-appimage",
        })
        self.assertEqual({upload['with']['path'] for upload in uploads}, {'${{ env.RELEASE }}/*.deb', '${{ env.RELEASE }}/*.AppImage'})
        # linuxdeploy's GTK plugin aborts without dpkg-architecture and the
        # gobject-introspection pkg-config file.
        packages = next(step for step in steps if step.get('name') == 'Install Linux build dependencies')['run']
        for package in ('dpkg-dev', 'libgirepository1.0-dev', 'patchelf'):
            self.assertIn(package, packages)
        action = yaml.safe_load((ROOT / '.github/actions/clap-resources/action.yml').read_text(encoding='utf-8'))
        linux = [step for step in action['runs']['steps'] if step.get('name') == 'Prepare and verify Linux worker']
        self.assertEqual(len(linux), 1)
        self.assertEqual(linux[0]['if'], "runner.os == 'Linux'")


if __name__ == '__main__': unittest.main()
