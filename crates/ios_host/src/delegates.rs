//! The application and scene delegates: `run` hands control to UIKit, the
//! scene delegate builds the window when the scene connects and forwards the
//! scene lifecycle to the [`Host`](crate::render::Host).

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class};
use objc2_foundation::{NSObjectProtocol, NSString};
use objc2_ui_kit::{
    UIApplication, UIApplicationDelegate, UIResponder, UIScene, UISceneConnectionOptions,
    UISceneDelegate, UISceneSession, UIViewController, UIWindow, UIWindowScene,
    UIWindowSceneDelegate,
};

use crate::view::MetalView;
use crate::{Factory, SCENE_DELEGATE_CLASS, render};

thread_local! {
    /// Taken when the scene connects (the egui context exists from then on).
    static FACTORY: RefCell<Option<Factory>> = const { RefCell::new(None) };
    /// UIKit does not keep the window alive for us.
    static WINDOW: RefCell<Option<Retained<UIWindow>>> = const { RefCell::new(None) };
}

/// Runs the app: UIKit's main loop, with [`SCENE_DELEGATE_CLASS`] as the
/// scene delegate (named by the Info.plist). `make` builds the app when the
/// scene connects. Must be called on the main thread; never returns.
pub fn run(make: Factory) -> ! {
    let mtm = MainThreadMarker::new().expect("pm_ios_host::run must be called on the main thread");
    FACTORY.with(|f| *f.borrow_mut() = Some(make));
    // objc2 registers classes on first use, but UIKit looks the scene
    // delegate up by name from the Info.plist: register it now.
    let scene_delegate = SceneDelegate::class();
    assert_eq!(scene_delegate.name().to_str(), Ok(SCENE_DELEGATE_CLASS));
    let app_delegate = NSString::from_class(AppDelegate::class());
    UIApplication::main(None, Some(&app_delegate), mtm)
}

define_class!(
    #[unsafe(super(UIResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "PMAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl UIApplicationDelegate for AppDelegate {}
);

define_class!(
    #[unsafe(super(UIResponder))]
    #[thread_kind = MainThreadOnly]
    // Keep equal to `SCENE_DELEGATE_CLASS` (checked in `run`).
    #[name = "PMSceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) {
            let Some(scene) = scene.downcast_ref::<UIWindowScene>() else {
                return;
            };
            let Some(make) = FACTORY.with(|f| f.borrow_mut().take()) else {
                return; // a second scene: the Info.plist allows only one
            };
            let mtm = self.mtm();
            let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
            let view = MetalView::new(mtm);
            view.setMultipleTouchEnabled(false);
            view.sync_scale(&scene.screen());
            let controller = UIViewController::new(mtm);
            controller.setView(Some(&view));
            window.setRootViewController(Some(&controller));
            window.makeKeyAndVisible();
            render::start(view, make);
            WINDOW.with(|w| *w.borrow_mut() = Some(window));
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, _scene: &UIScene) {
            render::with_host(|host| host.enter_background());
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, _scene: &UIScene) {
            render::with_host(|host| host.enter_foreground());
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn did_disconnect(&self, _scene: &UIScene) {
            render::stop();
            WINDOW.with(|w| w.borrow_mut().take());
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);
