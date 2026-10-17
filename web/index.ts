import * as THREE from "three";
import { GLTFLoader, type GLTF } from "three/addons/loaders/GLTFLoader.js";
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
// import { FirstPersonControls } from './FirstPersonControls.ts';
import STATS from 'three/addons/libs/stats.module.js';

/**
 * A camera with controls
 */
interface CameraWithControls extends THREE.Camera
{
    /**
     * Show that the camera has controls
     */
    isCameraWithControls: boolean;
}

class MainHandler
{
    // #region fields
    /**
     * The Window the canvas is in
     */
    private readonly window: Window;
    /**
     * The container of the canvas the 3D-Model should be rendered to
     */
    private readonly canvasContainer: HTMLElement;

    /**
     * The Canvas where the 3D-Model should be rendered to
     */
    private readonly canvas: HTMLCanvasElement;

    /**
     * The name of the mesh that acts as the main contianer that is filled
     */
    private readonly containerMeshName: string;

    /**
     * The name of the camera that acts as the position and fov of the lidar
     */
    private readonly lidarCameraName: string;

    /**
     * The main renderer
     */
    private readonly renderer: THREE.WebGLRenderer;

    /**
     * The main secene
     */
    private readonly mainScene: THREE.Scene;

    /**
     * The actions that should be called when cleaing up this class
     */
    private readonly cleanupActions: Array<() => void>;

    /**
     * Function with which the 3D-helper can be removed.
     */
    private readonly cleanupHelpers: Array<() => void>;

    /**
     * The cameras that can be used to display the 3d-model
     */
    private readonly cameras: Array<THREE.Camera>;
    /**
     * The registered Eventhandler for when the glb is loaded
     */
    private readonly glbLoadedEventHandlerList: Array<() => void>;

    /**
     * The registered Eventhandler for when the active camera changes
     */
    private readonly activeCameraChangeEventHandlerList: Array<() => void>;

    /**
     * The registered Eventhandler for when the loading of the glb is making progress
     */
    private readonly glbLoadingprogressEventHandlerList: Array<(loadingPercent: number) => void>;

    /**
     * The camera that is currently used to render the model
     */
    private activeCamera: THREE.Camera | null;

    /**
     * Hander for the display of statistics of the rendering
     */
    private stats: STATS | null;

    /**
     * Shows how much of the 3D-model is loaded. If null the model is loaded
     */
    private loadingPercent: number | null;

    /**
     * Controls to orbit the camera around a central point
     */
    private orbitControls: OrbitControls | null;

    /**
     * The Datetime when the last error was logged while running the game loop
     */
    private lastLoggedErrorInGameLoop: Date | null;

    /**
     * The main container mesh. Will only be filld after the glb is loaded.
     */
    private mainContainerMesh: THREE.Mesh| null;

    // /**
    //  * Controls to control the camera like in a FPS game
    //  */
    // private firstPersonControls: FirstPersonControls | null;
    // #endregion

    // #region ctor
    /**
     * Initialises the Class
     * 
     * @param window The window the canvas is in
     * @param canvasContainerQuery The query-selector for the container of the canvas where the 3D-model should be drawn
     * @param containerMeshName The name of the mesh that acts as the main contianer that is filled
     * @param lidarCameraName The name of the camera that acts as the position and fov of the lidar
     */
    public constructor(
        window: Window,
        canvasContainerQuery: string,
        containerMeshName: string,
        lidarCameraName: string)
    {
        this.cleanupActions = new Array<() => void>();
        this.cleanupHelpers = new Array<() => void>();
        this.cameras = new Array<THREE.Camera>();
        this.activeCamera = null;
        this.stats = null;
        this.loadingPercent = 0;
        this.orbitControls = null;
        // this.firstPersionControls = null;
        this.lastLoggedErrorInGameLoop = null;
        this.glbLoadedEventHandlerList = new Array<() => void>();
        this.activeCameraChangeEventHandlerList = new Array<() => void>();
        this.glbLoadingprogressEventHandlerList = new Array<() => void>();
        this.mainContainerMesh = null;

        this.window = window;
        this.containerMeshName = containerMeshName;
        this.lidarCameraName = lidarCameraName;

        let canvasContainer = document.querySelector<HTMLElement>(canvasContainerQuery);

        if (canvasContainer == null)
        {
            throw new Error(`The CanvasContainer "${canvasContainerQuery}" was not found.`);
        }

        this.canvasContainer = canvasContainer;

        let canvasList = this.canvasContainer.querySelectorAll("canvas");

        if (canvasList.length == 0)
        {
            throw new Error(`No Canvas in the CanvasContainer found.`);
        }
        else if (canvasList.length > 1)
        {
            console.warn(`Found ${canvasList.length} canvas in the CanvasContainer. Chossing the first and ignoring the rest...`);
        }

        let firstCanvas = canvasList[0];

        if (firstCanvas === undefined)
        {
            throw new Error("Unexcpected length while accessing the canvasList");
        }

        this.canvas = firstCanvas;

        this.renderer = new THREE.WebGLRenderer({ canvas: this.canvas, antialias: true, alpha: true });
        this.cleanupActions.push(() => {this.renderer.dispose();});
        this.mainScene = new THREE.Scene();
    }
    // #endregion
    
    // #region InitializeRender
    /**
     * Initialises the Renderer
     */
    public InitializeRender()
    {
        this.SetupStatsBox();

        this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
        this.renderer.toneMappingExposure = 1;

        console.debug("3D-model: Adding a ambient light at %i%...", 45);
        let ambientLight = new THREE.AmbientLight(0xFFFFFF, 0.45);
        this.mainScene.add(ambientLight);

        let handleOnResize = () => { this.HandleOnResize(); };
        this.window.addEventListener("resize", handleOnResize);
        this.cleanupActions.push(() => { this.window.removeEventListener("resize", handleOnResize); });

        new GLTFLoader().load(
            "/api/model3d",
            (data) => this.HandleOnGlbLoadingIsDone(data),
            (progressEvent: ProgressEvent) => this.HandleOnGlbLoadProgress(progressEvent),
            (error) => console.error("3D-Model: Error while loading the glb file: %o", error)
        );

        console.log(this.lidarCameraName);
        this.HandleOnResize();
    }
    // #endregion

    // #region Add3dHelper
    /**
     * Adds 3D-Helper to the scene
     */
    public Add3dHelper()
    {
        console.debug("3D-Model: Adding grid helper...");
        let gridHelper = new THREE.GridHelper(10, 1);
        this.mainScene.add(gridHelper);
        this.cleanupHelpers.push(() => {this.mainScene.remove(gridHelper);gridHelper.dispose();});

        console.debug("3D-Model: Adding axis helper...");
        let axisHelper = new THREE.AxesHelper(2);
        this.mainScene.add(axisHelper);
        this.cleanupHelpers.push(() => {this.mainScene.remove(axisHelper);axisHelper.dispose();});

        for (let directionalLight of this.mainScene.getObjectsByProperty("isDirectionalLight", true))
        {
            let helper = new THREE.DirectionalLightHelper(<THREE.DirectionalLight>directionalLight);
            this.mainScene.add(helper);
        }

        for (let pointLight of this.mainScene.getObjectsByProperty("isPointLight", true))
        {
            let helper = new THREE.PointLightHelper(<THREE.PointLight>pointLight);
            this.mainScene.add(helper);
        }

        for (let spotLight of this.mainScene.getObjectsByProperty("isSpotLight", true))
        {
            let helper = new THREE.SpotLightHelper(<THREE.Light>spotLight);
            this.mainScene.add(helper);
        }

        for (let hemisphereLight of this.mainScene.getObjectsByProperty("isHemisphereLight ", true))
        {
            let helper = new THREE.HemisphereLightHelper(<THREE.HemisphereLight>hemisphereLight, 5);
            this.mainScene.add(helper);
        }

        for (let skinnedMesh of this.mainScene.getObjectsByProperty("isBone", true))
        {
            let helper = new THREE.SkeletonHelper(skinnedMesh);
            this.mainScene.add(helper);
        }
    }
    // #endregion

    // #region Remove3dHelpers
    /**
     * Removes the 3D-Helpers from the scene
     */
    public Remove3dHelpers()
    {
        for (let action of this.cleanupHelpers)
        {
            action();
        }
        this.cleanupHelpers.length = 0;
    }
    // #endregion

    // #region AddGlbLoadedEventHandler
    /**
     * Registers a handler for when the glb is loaded
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddGlbLoadedEventHandler(handler: () => void): () => void
    {
        this.glbLoadedEventHandlerList.push(handler);

        return () => {
            let index = this.glbLoadedEventHandlerList.push(handler);

            if (index != -1)
            {
                this.glbLoadedEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region AddActiveCameraChangeEventHandler
    /**
     * Registers a handler for when the active camera changes
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddActiveCameraChangeEventHandler(handler: () => void): () => void
    {
        this.activeCameraChangeEventHandlerList.push(handler);

        return () => {
            let index = this.activeCameraChangeEventHandlerList.push(handler);

            if (index != -1)
            {
                this.activeCameraChangeEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region AddGlbLoadingprogessEventHandler
    /**
     * Registers a handler for when the loading of the glb is making progress
     * 
     * @param handler The handler to register
     * @returns Returns a action to unsubscribe
     */
    public AddGlbLoadingprogessEventHandler(handler: (loadingPercent: number) => void): () => void
    {
        this.glbLoadingprogressEventHandlerList.push(handler);

        return () => {
            let index = this.glbLoadingprogressEventHandlerList.push(handler);

            if (index != -1)
            {
                this.glbLoadingprogressEventHandlerList.splice(index);
            }
        };
    }
    // #endregion

    // #region ChangeCamera
    /**
     * Changes the Camera that is rendered
     * 
     * @param camera The camera that should be rended
     */
    public ChangeCamera(camera: THREE.Camera)
    {
        this.activeCamera = camera;
        this.UpdateCameraProjectionMatrixAndRatio(camera);
        this.SetupCameraControlsForSupporedCameras(camera);
    }
    // #endregion

    // #region CameraList
    /**
     * The cameras in the model
     */
    public get CameraList()
    {
        return this.cameras;
    }
    // #endregion

    // #region LogActiveCameraPosition
    /**
     * Logs the position of the active camera in the console
     */
    public LogActiveCameraPosition()
    {
        if (this.activeCamera == null)
        {
            console.info("3D-Model: Active Camera is null.");
        }
        else
        {
            console.info(
                "3D-Model: Active Camera-Position: x=%f; y=%f; z=%f; Camera-Rotations: x=%f; y=%f; z=%f;",
                this.activeCamera.position.x,
                this.activeCamera.position.y,
                this.activeCamera.position.z,
                this.activeCamera.rotation.x,
                this.activeCamera.rotation.y,
                this.activeCamera.rotation.z
            );
        }
    }
    // #endregion

    // #region CalculateMainContainerVolume
    /**
     * Calculates the Volume of the MainContainer
     * 
     * @returns Returns the Volume
     */
    public CalculateMainContainerVolume(): number
    {
        if (this.mainContainerMesh == null)
        {
            return 0;
        }

        return this.CalculateVolumeOfMesh(this.mainContainerMesh);
    }
    // #endregion

    // #region CalculateVolumeOfMesh
    /**
     * Calcaluates the volume of the mesh
     * 
     * @param mesh The mesh to calculate
     * @returns Returns the volume
     */
    private CalculateVolumeOfMesh(mesh: THREE.Mesh): number
    {
        const geometry = mesh.geometry;
        const position = geometry.attributes["position"];
        const index = geometry.index;

        if (position === undefined)
        {
            return 0;
        }

        let volume = 0;

        const a = new THREE.Vector3();
        const b = new THREE.Vector3();
        const c = new THREE.Vector3();

        const triangleCount = index
            ? index.count / 3
            : position.count / 3;

        for (let i = 0; i < triangleCount; i++) {
            const i0 = index ? index.getX(i * 3) : i * 3;
            const i1 = index ? index.getX(i * 3 + 1) : i * 3 + 1;
            const i2 = index ? index.getX(i * 3 + 2) : i * 3 + 2;

            a.fromBufferAttribute(position, i0);
            b.fromBufferAttribute(position, i1);
            c.fromBufferAttribute(position, i2);

            volume += a.dot(b.clone().cross(c)) / 6;
        }

        return Math.abs(volume);
    }
    // #endregion

    // #region RaiseGlbLoaded
    /**
     * Raises the event for when the glb is loaded
     */
    private RaiseGlbLoaded(): void
    {
        for (let handler of this.glbLoadedEventHandlerList)
        {
            handler();
        }
    }
    // #endregion

    // #region RaiseActiveCameraChange
    /**
     * Raises the event for when the active camera changes
     */
    private RaiseActiveCameraChange(): void
    {
        for (let handler of this.activeCameraChangeEventHandlerList)
        {
            handler();
        }
    }
    // #endregion

    // #region RaiseGlbLoadingprogess
    /**
     * Raises the event for when the loading of the glb is making progress
     * 
     * @param loadingPercent The percent the glb is loaded
     */
    private RaiseGlbLoadingprogess(loadingPercent: number): void
    {
        for (let handler of this.glbLoadingprogressEventHandlerList)
        {
            handler(loadingPercent);
        }
    }
    // #endregion

    // #region HandleOnResize
    /**
     * Is called when the size of the window changes.
     * It then sets the size and apsect ratio of the renderer and canvs.
     */
    private HandleOnResize()
    {
        this.SetRatioAndSizeInTheRenderer(
            this.window,
            this.canvasContainer.clientWidth,
            this.canvasContainer.clientHeight
        );

        if (this.activeCamera != null)
        {
            this.UpdateCameraProjectionMatrixAndRatio(this.activeCamera);
        }
    }
    // #endregion

    // #region HandleOnGlbLoadingIsDone
    /**
     * Is called when the glb is successfully loaded. Sets the rendering of it
     * up then and starts the game loop
     *
     * @param loadedGLB The glb that was loaded
     */
    private HandleOnGlbLoadingIsDone(loadedGLB: GLTF)
    {
        this.loadingPercent = null;

        console.debug("3D-Model: Glb was loaded. Adding it to scene...");
        this.mainScene.add(loadedGLB.scene);

        let mainContainerMesh = <THREE.Mesh>loadedGLB.scene.getObjectByName(this.containerMeshName);
        if (mainContainerMesh !== undefined) {
            this.mainContainerMesh = mainContainerMesh;
        }

        let center: THREE.Vector3;
        if (mainContainerMesh === null)
        {
            console.error(
                "3D-Model: The mesh of the container that is filled wan't found under the name \"%s\". Display will not work correctly.",
                this.containerMeshName
            );
            center = new THREE.Vector3(0, 0, 0);
        }
        else
        {
            const box = new THREE.Box3().setFromObject(mainContainerMesh);
            center = box.getCenter(new THREE.Vector3());
        }

        this.cameras.length = 0;

        let perspectiveCamera = new THREE.PerspectiveCamera(75, this.GetAspectRatio(), 0.1, 2000);
        perspectiveCamera.name = "Haupt-Kamera";
        perspectiveCamera.position.set(-0.345346, 2.500441, -3.571497);
        perspectiveCamera.lookAt(center);
        perspectiveCamera.updateProjectionMatrix();
        (<CameraWithControls><unknown>perspectiveCamera).isCameraWithControls = true;
        this.mainScene.add(perspectiveCamera);
        this.cameras.push(perspectiveCamera);
        this.activeCamera = perspectiveCamera;

        let orthographicCamera = new THREE.OrthographicCamera();
        orthographicCamera.name = "Orthogonal-Kamera";
        orthographicCamera.position.set(-1, 3, -1);
        orthographicCamera.lookAt(center);
        orthographicCamera.updateProjectionMatrix();
        (<CameraWithControls><unknown>orthographicCamera).isCameraWithControls = true;
        this.mainScene.add(orthographicCamera);
        this.cameras.push(orthographicCamera);

        this.UpdateCameraProjectionMatrixAndRatio(this.activeCamera);
        this.SetupCameraControlsForSupporedCameras(this.activeCamera);

        // Changing the MainContainer Mesh to only render the front side (which should face inwards)
        const materials = Array.isArray(mainContainerMesh.material)
            ? mainContainerMesh.material
            : [mainContainerMesh.material];
        for (const material of materials) {
            material.side = THREE.FrontSide;
            material.needsUpdate = true;
        }

        this.GameLoop();
        this.RaiseGlbLoaded();
        this.RaiseActiveCameraChange();
    }
    // #endregion

    // #region UpdateCameraProjectionMatrixAndRatio
    /**
     * Updates the cameras projection matrix and the aspect ratio
     * 
     * @param camera The camera to update
     */
    private UpdateCameraProjectionMatrixAndRatio(camera: THREE.Camera): void
    {
        console.debug("3D-Model: Updating the matrix/ratio of the camera...");

        let aspectRatio = this.GetAspectRatio();

        if ((<THREE.PerspectiveCamera>camera).isPerspectiveCamera == true)
        {
            let castedCamera = <THREE.PerspectiveCamera>camera;

            console.debug("3D-Model: Setting the aspect ratio of %f to the perspective camera...", aspectRatio);
            castedCamera.aspect = aspectRatio;
            castedCamera.updateProjectionMatrix();
        }
        else if ((<THREE.OrthographicCamera>camera).isOrthographicCamera == true)
        {
            let castedCamera = <THREE.OrthographicCamera>camera;

            let heightOfCamera = castedCamera.top - castedCamera.bottom;
            let centerX = (castedCamera.left + castedCamera.right) / 2;

            // Change the width, height of the camera to the aspect ratio but
            // keep the center where it is
            let newWidth = heightOfCamera * aspectRatio;
            castedCamera.right = centerX + (newWidth / 2);
            castedCamera.left = centerX - (newWidth / 2);

            console.debug("3D-Model: The new camera-size is %fx%f (aspectRatio: %f).", newWidth, heightOfCamera, aspectRatio);
        }
        else
        {
            console.warn("The camera-type is unknown (%o). Can't update ratio.", camera);
        }
    }
    // #endregion

    // #region GetAspectRatio
    /**
     * Gets the aspect ratio for the canvas
     * 
     * @returns Returns the ratio
     */
    private GetAspectRatio()
    {
        return this.canvasContainer.clientWidth / this.canvasContainer.clientHeight;
    }
    // #endregion

    // #region SetRatioAndSizeInTheRenderer
    /**
     * Sets the aspect ratio and the dimensions of the renderer.
     * Also sets the devicePixelRatio in the render.
     * 
     * @param window The window in which the renderer is
     * @param renderWidth The width of the rendered image (not how big it should be)
     * @param renderHeight The height of the rendered image (not how big it should be)
     */
    private SetRatioAndSizeInTheRenderer(window: Window, renderWidth: number, renderHeight: number): void
    {
        this.canvas.removeAttribute("style");
        this.canvas.setAttribute("height", renderHeight.toString());
        this.canvas.setAttribute("width", renderWidth.toString());

        console.debug("3D-Model: Setting the devicePixelRatio to %f...", window.devicePixelRatio);
        this.renderer.setPixelRatio(window.devicePixelRatio);
        console.debug("3D-Model: Changing the size of the renderer to %i x %i", renderWidth, renderHeight);
        this.renderer.setSize(renderWidth, renderHeight);
    }
    // #endregion

    // #region SetupStatsBox
    /**
     * Sets the stats box up to display the render stats
     */
    private SetupStatsBox()
    {
        this.stats = new STATS();
        this.stats.showPanel(0)

        this.canvasContainer.appendChild(this.stats.dom);

        // Move the stats-box to be in the top left corner of the container-element
        let containerBoundingBox = this.canvasContainer.getBoundingClientRect();
        
        this.stats.dom.style.left = `${containerBoundingBox.x}px`;
        this.stats.dom.style.top = `${containerBoundingBox.y}px`;
    }
    // #endregion

    // #region SetupCameraControlsForSupporedCameras
    /**
     * Sets camera controls up for the camera if it supports it
     *
     * @param camera The camera to setup the controls for
     */
    private SetupCameraControlsForSupporedCameras(camera: THREE.Camera)
    {
        this.orbitControls?.dispose();
        this.orbitControls = null;

        // this.firstPersonControls?.dispose();
        // this.firstPersonControls = null;

        if ((<CameraWithControls><unknown>camera).isCameraWithControls !== undefined &&
            (<CameraWithControls><unknown>camera).isCameraWithControls)
        {
            this.orbitControls = new OrbitControls(camera, this.renderer.domElement);
            
            if (this.mainContainerMesh != null)
            {
                const box = new THREE.Box3().setFromObject(this.mainContainerMesh);
                let center = box.getCenter(new THREE.Vector3());
                center.y -= 0.5;
                this.orbitControls.target = center;
            }

            // this.firstPersonControls = new FirstPersonControls(camera, this.renderer.domElement);
            // this.firstPersonControls.Enabled = false;
            // this.firstPersonControls.MouseLookSpeed = 15;
            // this.firstPersonControls.MovementSpeed = 15;
            // this.firstPersonControls.ControllerLookSpeed = 50;
        }
    }
    // #endregion

    // #region HandleOnGlbLoadProgress
    /**
     * Is Called when the GLB-Loading is making progress
     * 
     * @param event The loading event
     */
    private HandleOnGlbLoadProgress(event: ProgressEvent)
    {
        let progress = Math.round(event.loaded / event.total * 100);

        console.debug("3D-Model: GLTF loading at %f%", progress);
        this.loadingPercent = progress;

        this.RaiseGlbLoadingprogess(this.loadingPercent);
    }
    // #endregion

    // #region GameLoop
    /**
     * Starts the Gameloop in which the model is rendered to the canvas
     */
    private GameLoop(): void
    {
        try
        {
            if (this.activeCamera != null)
            {
                this.stats?.begin();
                if (this.orbitControls?.enabled)
                {
                    this.orbitControls.update();
                }
                // if (this.firstPersonControls?.enabled)
                // {
                //     this.firstPersonControls.update();
                // }

                this.renderer.render(this.mainScene, this.activeCamera);

                this.stats?.end();
            }
        }
        catch(error: any)
        {
            if (this.lastLoggedErrorInGameLoop == null ||
                new Date().getTime() - this.lastLoggedErrorInGameLoop.getTime() > 2000)
            {
                console.error("3D-Model: Error while running the gameloop: %o", error);
                this.lastLoggedErrorInGameLoop = new Date();
            }
        }
        finally
        {
            requestAnimationFrame(() => { this.GameLoop(); });
        }
    }
    // #endregion
}

let overlayContainerQuery = ".Overlay";
let loadingProgressQuery = ".load-process";
let loadingTextQuery = ".load-label-percent";
let volumeTextElementQuery = ".StatsBox.Volume .Value";
let maxVolumeTextElementQuery = ".StatsBox.Volume .MaxValue";

let overlayContainer = document.querySelector<HTMLElement>(overlayContainerQuery);

if (overlayContainer == null)
{
    throw new Error(`The OverlayContainer "${overlayContainerQuery} was not found."`);
}

let progressElement = overlayContainer.querySelector<HTMLElement>(loadingProgressQuery);

if (progressElement == null)
{
    throw new Error(`The Progress "${loadingProgressQuery} was not found."`);
} 
else if (progressElement instanceof HTMLProgressElement == false)
{
    console.warn(`The Progress-Element "${loadingProgressQuery} wasn't a HTMLProgressElement. Might not work..."`);
}

let loadingTextElement = overlayContainer.querySelector<HTMLElement>(loadingTextQuery);

if (loadingTextElement == null)
{
    throw new Error(`The Progress "${loadingTextQuery} was not found."`);
}

let volumeTextElement = document.querySelector<HTMLElement>(volumeTextElementQuery);

if (volumeTextElement == null)
{
    throw new Error(`The VolumeTextElement "${volumeTextElementQuery} was not found."`);
}

let maxVolumeTextElement = document.querySelector<HTMLElement>(maxVolumeTextElementQuery);

if (maxVolumeTextElement == null)
{
    throw new Error(`The MaxVolumeTextElement "${maxVolumeTextElementQuery} was not found."`);
}

let mainHandler = new MainHandler(window, ".CanvasContainer", "Main", "Camera");
(<any>window).MainHandler = mainHandler;

let numberFormatter = new Intl.NumberFormat(
    undefined,
    {
        minimumFractionDigits: 1,
        maximumFractionDigits: 1
    }
);

mainHandler.AddGlbLoadingprogessEventHandler((loadingProgess) => {
    if (loadingProgess < 100) {
        overlayContainer.classList.remove("Hide");
    } else {
        overlayContainer.classList.add("Hide");
    }

    progressElement.setAttribute("value", loadingProgess.toString());
    loadingTextElement.innerText = loadingProgess.toString();
});

mainHandler.AddGlbLoadedEventHandler(()  => {
    maxVolumeTextElement.innerText = numberFormatter.format(mainHandler.CalculateMainContainerVolume());
});

mainHandler.InitializeRender();