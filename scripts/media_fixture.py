"""Real video/image pixels in isolated native GUI sessions."""
from pathlib import Path
from PIL import Image
import os,subprocess,time

RED=(224,40,30)
GREEN=(32,208,64)
BLUE=(30,70,240)

def video(path,color=GREEN):
    temporary=path.with_name('next.mp4')
    value='0x'+''.join(f'{c:02x}' for c in color)
    subprocess.run(['ffmpeg','-v','error','-f','lavfi','-i',f'color=c={value}:s=320x180:d=1','-c:v','mpeg4','-threads','1','-y',str(temporary)],check=True)
    os.replace(temporary,path)

def prepare(base):
    source=base/'media-files';source.mkdir()
    video(source/'a-video.mp4')
    Image.new('RGB',(320,180),RED).save(source/'b-image.png')
    (source/'c.txt').write_text('Text after media preview')
    return source

def count(image,color,preview=False):
    region=(1072,200,1352,760) if preview else (200,170,1060,760)
    return sum(all(abs(a-b)<12 for a,b in zip(pixel,color)) for pixel in image.convert('RGB').crop(region).getdata())

def assert_aspect(image,color):
    crop=image.convert('RGB').crop((200,150,1060,300))
    mask=Image.frombytes('L',crop.size,bytes(255 if all(abs(a-b)<12 for a,b in zip(pixel,color)) else 0 for pixel in crop.getdata()))
    box=mask.getbbox();assert box,'No thumbnail pixels'
    width,height=box[2]-box[0],box[3]-box[1]
    assert abs(width/height-16/9)<.15,f'Thumbnail aspect changed: {box}'

def exercise(source,key,click,capture,wait_for,settings):
    def shot():return capture('current')
    wait_for(lambda: count(shot(),GREEN,True)>20000 and count(shot(),GREEN)>150 and count(shot(),RED)>150,'native video frame and image/video grid thumbnails')
    assert_aspect(capture('grid'),GREEN)
    assert_aspect(capture('grid'),RED)
    key('g','ctrl')
    wait_for(lambda:count(shot(),GREEN)>50 and count(shot(),RED)>50,'native list thumbnails')
    assert_aspect(capture('list'),GREEN)
    key('F10');click(62,1);time.sleep(.2);capture('settings')
    click(145,5);key('Escape');time.sleep(.3)
    assert count(shot(),GREEN)==0 and count(shot(),RED)==0,'thumbnails remained after setting disabled'
    assert count(shot(),GREEN,True)>20000,'thumbnail setting disabled video preview'
    key('F10');click(62,1);click(145,5);click(145,3);key('Escape');time.sleep(.3)
    assert count(shot(),GREEN)==0,'symbol icons unexpectedly show thumbnails'
    assert count(shot(),GREEN,True)>20000,'symbol icons disabled video preview'
    key('F10');click(62,1);click(145,3);key('Escape')
    wait_for(lambda:count(shot(),GREEN)>50,'thumbnails after re-enabling images')
    video(source/'a-video.mp4',BLUE)
    wait_for(lambda:count(shot(),BLUE,True)>20000 and count(shot(),BLUE)>50 and count(shot(),GREEN,True)==0,'watched same-path video rewrite')
    capture('rewrite')
    key('End');time.sleep(.3)
    assert count(shot(),BLUE,True)==0,'video frame remained over text preview'
    key('Home');wait_for(lambda:count(shot(),BLUE,True)>20000,'return to video')
    key('F3');time.sleep(.3)
    assert count(shot(),BLUE,True)==0,'video frame remained after preview disabled'
    capture('preview-off')
    print('Media GUI: video frame, grid/list thumbnails, mouse settings, symbol icons, watched rewrite, text and F3 passed',flush=True)
